//! A question a drag in the dock asks of the keyboard: is Escape down. The dock's windows are never activated, so no key event reaches
//! them; the state is read while a drag is held. Nothing is sent or hooked, only read.

use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};

/// Whether Escape is held down at this moment.
pub fn escape_down() -> bool {
    (unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000) != 0
}
