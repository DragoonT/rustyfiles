//! The point of the last right click on the empty desktop: what "New" and "Paste" of that menu mean by "here". Pure.
//!
//! The point belongs to ONE menu use. Whatever else happens (the menu is dismissed, an icon is clicked, a key is pressed) must not leave
//! it for a later Ctrl+V or Ctrl+Shift+N to find, so the rule is written down once: a command takes the point by saying how it was
//! started, and the point is spent either way.

use std::time::{Duration, Instant};

/// How long a right click's point is worth after the click (a menu is not left open for longer than this).
pub const FRESH: Duration = Duration::from_secs(60);

/// How a command was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// From an entry of the empty desktop's menu: "here" is where the right click was.
    Menu,
    /// From a key (Ctrl+V, Ctrl+Shift+N): the first free cells, whatever was clicked before.
    Keyboard,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MenuPoint(Option<((i32, i32), Instant)>);

impl MenuPoint {
    /// A right click on the empty desktop at `p` (canvas pixels).
    pub fn set(&mut self, p: (i32, i32), now: Instant) {
        self.0 = Some((p, now));
    }

    /// Something else was done (a press, the menu of a key, Escape): the point is not for any later command.
    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// The point a command started `via` may use; spent by asking, whatever the answer.
    pub fn take(&mut self, via: Via, now: Instant) -> Option<(i32, i32)> {
        let held = self.0.take();
        match via {
            Via::Keyboard => None,
            Via::Menu => held.filter(|(_, at)| now.saturating_duration_since(*at) < FRESH).map(|(p, _)| p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_command_gets_the_point_once() {
        let t0 = Instant::now();
        let mut m = MenuPoint::default();
        m.set((10, 20), t0);
        assert_eq!(m.take(Via::Menu, t0 + Duration::from_secs(3)), Some((10, 20)));
        assert_eq!(m.take(Via::Menu, t0 + Duration::from_secs(4)), None, "spent");
    }

    #[test]
    fn a_key_never_gets_the_point_and_spends_it() {
        let t0 = Instant::now();
        let mut m = MenuPoint::default();
        m.set((10, 20), t0);
        assert_eq!(m.take(Via::Keyboard, t0), None, "Ctrl+V after a right click goes to the first free cell");
        assert_eq!(m.take(Via::Menu, t0), None, "and the old point is not kept for a later menu command either");
    }

    #[test]
    fn a_dismissed_menu_does_not_leave_a_point_for_the_next_command() {
        let t0 = Instant::now();
        let mut m = MenuPoint::default();
        m.set((1, 2), t0);
        m.clear(); // an icon was pressed, or the menu was opened from the keyboard
        assert_eq!(m.take(Via::Menu, t0), None);
    }

    #[test]
    fn a_point_older_than_a_minute_is_stale() {
        let t0 = Instant::now();
        let mut m = MenuPoint::default();
        m.set((5, 5), t0);
        assert_eq!(m.take(Via::Menu, t0 + FRESH), None);
        m.set((5, 5), t0);
        assert_eq!(m.take(Via::Menu, t0 + FRESH - Duration::from_millis(1)), Some((5, 5)));
    }

    #[test]
    fn a_newer_right_click_replaces_the_older_point() {
        let t0 = Instant::now();
        let mut m = MenuPoint::default();
        m.set((1, 1), t0);
        m.set((2, 2), t0 + Duration::from_secs(1));
        assert_eq!(m.take(Via::Menu, t0 + Duration::from_secs(2)), Some((2, 2)));
    }
}
