//! Which presses on the cover window may activate it. Pure.
//!
//! WHAT WAS CHOSEN, AND WHY. Explorer's desktop is activatable: a click on it makes it the foreground window and the other windows
//! stay where they are, so Delete, F2 and Ctrl+C go to the icons. The research note says to keep that (no `WS_EX_NOACTIVATE`), and
//! so does this module for the part of the window that holds icons: a press on an ICON activates the window, because the keyboard
//! must act on the icon that was just chosen.
//!
//! A press on EMPTY space does not activate it (`MA_NOACTIVATE`): the press still reaches the view (it clears the selection and starts
//! the rubber band) but the user's window keeps the foreground, so a click on the empty desktop never takes the keyboard from the
//! program they were typing in. When the rubber band ends with icons selected the view activates the window then (the user has
//! chosen icons by hand, and the keyboard now belongs to them): that is a call in the view, not a decision here.

use crate::desktop::geom::Rect;

/// `WM_MOUSEACTIVATE` replies.
pub const MA_ACTIVATE: isize = 1;
pub const MA_NOACTIVATE: isize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    Activate,
    NoActivate,
}

impl Activation {
    /// The value the window procedure returns for `WM_MOUSEACTIVATE`.
    pub fn reply(self) -> isize {
        match self {
            Activation::Activate => MA_ACTIVATE,
            Activation::NoActivate => MA_NOACTIVATE,
        }
    }
}

/// `cursor`: the pointer in the window's client pixels. `hot`: the click areas of the icons, the same pixels. Before the view has
/// told the window where its icons are (`hot` empty and `known` false) every press activates, as an ordinary window would: a wrong
/// guess in that direction costs a click that takes the foreground, a wrong guess in the other would cost an icon that cannot be
/// used from the keyboard.
pub fn on_mouse_activate(cursor: (i32, i32), hot: &[Rect], known: bool) -> Activation {
    if !known || hot.iter().any(|r| r.contains(cursor.0, cursor.1)) { Activation::Activate } else { Activation::NoActivate }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_on_an_icon_activates() {
        let hot = [Rect::new(10, 10, 90, 100), Rect::new(10, 110, 90, 200)];
        assert_eq!(on_mouse_activate((50, 50), &hot, true), Activation::Activate);
        assert_eq!(on_mouse_activate((50, 150), &hot, true), Activation::Activate);
        assert_eq!(on_mouse_activate((10, 10), &hot, true), Activation::Activate, "the top-left pixel is inside");
    }

    #[test]
    fn a_press_on_empty_space_does_not() {
        let hot = [Rect::new(10, 10, 90, 100)];
        assert_eq!(on_mouse_activate((500, 500), &hot, true), Activation::NoActivate);
        assert_eq!(on_mouse_activate((90, 50), &hot, true), Activation::NoActivate, "the right edge is outside");
        assert_eq!(on_mouse_activate((50, 100), &hot, true), Activation::NoActivate, "the bottom edge is outside");
    }

    #[test]
    fn a_desktop_with_no_icons_known_to_be_empty_never_activates() {
        assert_eq!(on_mouse_activate((5, 5), &[], true), Activation::NoActivate);
    }

    #[test]
    fn before_the_view_has_said_where_its_icons_are_every_press_activates() {
        assert_eq!(on_mouse_activate((5, 5), &[], false), Activation::Activate);
    }

    #[test]
    fn the_replies_are_the_win32_values() {
        assert_eq!(Activation::Activate.reply(), 1);
        assert_eq!(Activation::NoActivate.reply(), 3);
    }
}
