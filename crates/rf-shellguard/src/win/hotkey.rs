//! The escape chord (Ctrl+Alt+Shift+E): a registered hotkey and a low-level keyboard hook, both on
//! one thread of their own that does nothing else. Either one signals the same event (named, so a
//! `--panic` process can signal it too), which `Processes::wait` waits on, so the guard needs no polling.
//! The event is MANUAL-reset: a wait that is satisfied by an auto-reset event resets it, and the
//! runner then asks `take_pressed` whether the chord was pressed, which would find the event already
//! used up and the press lost. `take_pressed` is what resets it. The hook callback only reads key states and sets an event: it never
//! blocks and allocates nothing (Windows silently drops a slow hook).
//! NOT RUN by the author of this crate (registering a global hotkey is forbidden in the build session).

use std::sync::atomic::Ordering;

use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, VK_CONTROL, VK_LMENU, VK_RMENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_SYSKEYDOWN};
use windows::core::PCWSTR;

use super::{signals, wide};
use crate::sys::Hotkey;

const VK_E: u32 = 0x45;
const HOTKEY_ID: i32 = 0x5247; // "RG"

pub struct RealHotkey {
    event_name: String,
    event: Option<isize>,
}

impl RealHotkey {
    pub fn new(event_name: &str) -> RealHotkey {
        RealHotkey { event_name: event_name.to_string(), event: None }
    }
}

fn down(vk: u16) -> bool {
    // SAFETY: reads a key state.
    unsafe { (GetAsyncKeyState(i32::from(vk)) as u16) & 0x8000 != 0 }
}

/// Ctrl+Alt+Shift with the LEFT Alt. On layouts that have an AltGr key (the Turkish Q keyboard among them) AltGr
/// is reported as Ctrl plus the right Alt, so a plain "Ctrl and Alt are down" test would fire while the user types
/// a character with AltGr+Shift+E, and a fired chord deletes the Shell value for good. The left Alt alone is the
/// real Alt; with the right Alt down the key is AltGr (or the user holds both) and the chord does not count.
pub fn chord_pressed(is_down: &dyn Fn(u16) -> bool) -> bool {
    is_down(VK_CONTROL.0) && is_down(VK_SHIFT.0) && is_down(VK_LMENU.0) && !is_down(VK_RMENU.0)
}

/// For the `RegisterHotKey` backup: it matches Ctrl+Alt+Shift+E for AltGr too, and keys may already be up again when
/// the message is read, so only the one sign of AltGr is checked here.
pub fn hotkey_message_counts(is_down: &dyn Fn(u16) -> bool) -> bool {
    !is_down(VK_RMENU.0)
}

fn signal() {
    let h = signals::WAKE.load(Ordering::SeqCst);
    if h != 0 {
        // SAFETY: the event stays open for the life of the process.
        unsafe {
            let _ = SetEvent(HANDLE(h as *mut _));
        }
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && (wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN) {
        // SAFETY: for a WH_KEYBOARD_LL hook with code >= 0, lparam points to a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.vkCode == VK_E && chord_pressed(&down) {
            signal();
        }
    }
    // Never swallow the key: the chord is only a signal.
    // SAFETY: passes the arguments on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn thread_main() {
    // SAFETY: a hotkey with no window belongs to this thread and arrives as WM_HOTKEY in its queue.
    unsafe {
        let _ = RegisterHotKey(None, HOTKEY_ID, HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_SHIFT.0 | MOD_NOREPEAT.0), VK_E);
    }
    // SAFETY: `hook_proc` is a plain function; the module handle is this exe's.
    let _hook: Option<HHOOK> = unsafe {
        let module = GetModuleHandleW(None).ok();
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module.map(|m| windows::Win32::Foundation::HINSTANCE(m.0)), 0).ok()
    };
    let mut msg = MSG::default();
    // SAFETY: the standard message loop; it ends only when the process does.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY && hotkey_message_counts(&down) {
                signal();
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

impl RealHotkey {
    /// Creates the escape event and nothing else: no hotkey is registered, no hook is installed, no thread starts.
    /// `arm` is this plus the thread; tests use it alone to drive the event the way the thread and a `--panic`
    /// process would.
    pub fn create_event_only(&mut self) -> Result<(), String> {
        if self.event.is_some() {
            return Ok(());
        }
        let name = wide(&self.event_name);
        // SAFETY: `name` is NUL-terminated; MANUAL-reset (see the module comment), not signalled.
        let h = unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) }.map_err(|e| e.to_string())?;
        signals::WAKE.store(h.0 as isize, Ordering::SeqCst);
        self.event = Some(h.0 as isize);
        Ok(())
    }
}

impl Hotkey for RealHotkey {
    fn arm(&mut self) -> Result<(), String> {
        if self.event.is_some() {
            return Ok(());
        }
        self.create_event_only()?;
        let h = HANDLE(self.event.unwrap_or(0) as *mut _);
        std::thread::Builder::new().name("escape-chord".into()).spawn(thread_main).map_err(|e| {
            // SAFETY: closing the event just created.
            unsafe {
                let _ = CloseHandle(h);
            }
            self.event = None;
            signals::WAKE.store(0, Ordering::SeqCst);
            e.to_string()
        })?;
        Ok(())
    }

    fn waitable(&self) -> isize {
        self.event.unwrap_or(0)
    }

    fn take_pressed(&mut self) -> bool {
        match self.event {
            // SAFETY: the event is open. Manual-reset: a zero wait only looks, and the reset below consumes the press. A
            // press that lands between the look and the reset is the same press (the guard is about to act on it).
            Some(h) => unsafe {
                let handle = HANDLE(h as *mut _);
                let pressed = WaitForSingleObject(handle, 0) == WAIT_OBJECT_0;
                if pressed {
                    let _ = ResetEvent(handle);
                }
                pressed
            },
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(down: &'static [u16]) -> impl Fn(u16) -> bool {
        move |vk| down.contains(&vk)
    }

    #[test]
    fn the_chord_is_ctrl_shift_and_the_left_alt() {
        assert!(chord_pressed(&keys(&[VK_CONTROL.0, VK_SHIFT.0, VK_LMENU.0])));
        assert!(!chord_pressed(&keys(&[VK_CONTROL.0, VK_SHIFT.0])));
        assert!(!chord_pressed(&keys(&[VK_CONTROL.0, VK_LMENU.0])));
        assert!(!chord_pressed(&keys(&[VK_SHIFT.0, VK_LMENU.0])));
        assert!(!chord_pressed(&keys(&[])));
    }

    #[test]
    fn altgr_with_shift_is_not_the_chord() {
        // AltGr is reported as Ctrl plus the right Alt: the Turkish Q keyboard types characters with it.
        assert!(!chord_pressed(&keys(&[VK_CONTROL.0, VK_SHIFT.0, VK_RMENU.0])));
        // The generic Alt key reads as down for either side; the right one being down rules it out.
        assert!(!chord_pressed(&keys(&[VK_CONTROL.0, VK_SHIFT.0, VK_LMENU.0, VK_RMENU.0])));
    }

    #[test]
    fn the_hotkey_backup_only_refuses_altgr() {
        assert!(hotkey_message_counts(&keys(&[])));
        assert!(hotkey_message_counts(&keys(&[VK_CONTROL.0, VK_SHIFT.0, VK_LMENU.0])));
        assert!(!hotkey_message_counts(&keys(&[VK_CONTROL.0, VK_SHIFT.0, VK_RMENU.0])));
    }
}
