//! The keyboard feed of the chord: the thing that sees Alt+Tab before the program in front does and tells the switcher.
//!
//! NOTHING IN THIS TRACK INSTALLS IT. The trait [`KeyFeed`] is what the shell host will use; [`LowLevelHook`] is the real
//! implementation (a `WH_KEYBOARD_LL` hook on a thread of its own), written, compiled and tested only through [`translate`] (the
//! pure part). `start` of the real one is called by no code that runs; tests and probes use [`FakeFeed`].
//!
//! WHAT THE REAL HOOK MUST BE (docs for `LowLevelKeyboardProc`, research note 4-desktop.md 6.4):
//! * the callback runs on the thread that installed the hook, which must pump messages; it must answer within
//!   `LowLevelHooksTimeout` (at most 1000 ms) or Windows removes the hook WITHOUT telling it: so the callback only translates the
//!   event, asks the chord (a few comparisons) and posts the answer to the UI thread; it never blocks and never allocates much;
//! * the answer to "swallow?" is the callback's return value (nonzero swallows), decided by the chord from the event alone;
//! * Shift and Ctrl are known from the hook's OWN key events ([`Mods`]), not from `GetKeyState`: the hook thread reads no keyboard
//!   messages, and `GetKeyState` follows the messages a thread has read, so on it Shift+Tab could arrive as a plain Tab. The state
//!   is seeded once from `GetAsyncKeyState` when the hook starts (a modifier already held then);
//! * the "same app" key is found by its SCAN CODE (the key above Tab, 0x29, on any layout), not by `VK_OEM_3`, which sits on a
//!   typing key of other layouts; and a key pressed with Ctrl down is not the chord's: AltGr is reported as a right Alt plus a
//!   Ctrl that Windows makes up, so AltGr + a letter must never open the switcher or be swallowed;
//! * `mask_alt` means "one harmless key must go in before this Alt release" (see `chord.rs`). The callback below hands the whole
//!   [`Reaction`] to the host, which decides how (`SendInput` of `VK_E8` down and up is the usual way; it comes back to the hook as
//!   an injected event, which the chord ignores). Whether such a key sent from inside the hook lands BEFORE the release being
//!   processed is not known (UNV); the host's first live test settles it. Nothing here sends a key.
//! * the thread, and with it the hook, is stopped by posting `WM_QUIT`, and a hook that was removed by Windows is noticed by a
//!   heartbeat (an event the hook itself generates) and installed again: that part belongs to the host, not here.

use std::sync::Arc;

use super::chord::{Chord, Key, KeyInput, Reaction};

/// What a feed is asked to call for each key event: it answers whether the key is swallowed.
pub type OnKey = Box<dyn FnMut(KeyInput) -> bool + Send>;

/// Something that delivers key events to a callback until it is stopped.
pub trait KeyFeed {
    fn start(&mut self, on_key: OnKey) -> anyhow::Result<()>;
    fn stop(&mut self);
    fn running(&self) -> bool;
}

/// The chord wired to a callback: the function a feed calls for each event. `on_reaction` is called for every event that has an
/// effect or needs the Alt mask, with the whole reaction, and must return at once (post it to the UI thread).
pub fn chord_callback(chord: Arc<parking_lot::Mutex<Chord>>, on_reaction: impl Fn(Reaction) + Send + 'static) -> OnKey {
    Box::new(move |k| {
        let r: Reaction = chord.lock().feed(k);
        if r.effect.is_some() || r.mask_alt {
            on_reaction(r);
        }
        r.swallow
    })
}

// ---------------------------------------------------------------- key codes (pure)

pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_LSHIFT: u32 = 0xA0;
pub const VK_RSHIFT: u32 = 0xA1;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_MENU: u32 = 0x12;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;
/// The scan code of the key above Tab (the back quote on a US keyboard), whatever the layout puts on it.
pub const SCAN_ABOVE_TAB: u32 = 0x29;
/// `KBDLLHOOKSTRUCT.flags`: Alt is down / the event was injected.
pub const LLKHF_ALTDOWN: u32 = 0x20;
pub const LLKHF_INJECTED: u32 = 0x10;

/// The chord's key for a virtual-key code and the key's scan code (the "same app" key is found by the scan code).
pub fn key_of(vk: u32, scan: u32) -> Key {
    match vk {
        _ if scan == SCAN_ABOVE_TAB && !is_modifier(vk) => Key::Grave,
        VK_TAB => Key::Tab,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_MENU | VK_LMENU | VK_RMENU => Key::Alt,
        _ => Key::Other,
    }
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_MENU | VK_LMENU | VK_RMENU)
}

/// Which Shift and Ctrl keys are down, kept from the hook's own key events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    lshift: bool,
    rshift: bool,
    lctrl: bool,
    rctrl: bool,
}

impl Mods {
    /// The state when the hook starts (`GetAsyncKeyState` of each key, read by the caller).
    pub fn seeded(lshift: bool, rshift: bool, lctrl: bool, rctrl: bool) -> Mods {
        Mods { lshift, rshift, lctrl, rctrl }
    }

    /// One key event. The generic codes (`VK_SHIFT`, `VK_CONTROL`) are not sent to a low-level hook, but count if they come.
    pub fn observe(&mut self, vk: u32, down: bool) {
        match vk {
            VK_LSHIFT | VK_SHIFT => self.lshift = down,
            VK_RSHIFT => self.rshift = down,
            VK_LCONTROL | VK_CONTROL => self.lctrl = down,
            VK_RCONTROL => self.rctrl = down,
            _ => {}
        }
    }

    pub fn shift(&self) -> bool {
        self.lshift || self.rshift
    }

    pub fn ctrl(&self) -> bool {
        self.lctrl || self.rctrl
    }
}

/// A hook event as the chord wants it: `vk`, `scan` and `flags` from the `KBDLLHOOKSTRUCT`, `down` from the message
/// (`WM_KEYDOWN` and `WM_SYSKEYDOWN` are down), the modifiers from [`Mods`]. A key pressed with Ctrl down is another key as far as
/// the chord goes (Ctrl+Alt+Tab is Windows' own sticky switcher; AltGr is a right Alt with a made-up Ctrl): Alt itself still passes,
/// so that its release is seen.
pub fn translate(vk: u32, scan: u32, flags: u32, down: bool, mods: Mods) -> KeyInput {
    let key = match key_of(vk, scan) {
        Key::Alt => Key::Alt,
        _ if mods.ctrl() => Key::Other,
        k => k,
    };
    KeyInput { key, down, shift: mods.shift(), alt: flags & LLKHF_ALTDOWN != 0, injected: flags & LLKHF_INJECTED != 0 }
}

// ---------------------------------------------------------------- the real hook (never started in this track)

pub struct LowLevelHook {
    thread: Option<std::thread::JoinHandle<()>>,
    thread_id: u32,
}

impl LowLevelHook {
    pub fn new() -> LowLevelHook {
        LowLevelHook { thread: None, thread_id: 0 }
    }
}

impl Default for LowLevelHook {
    fn default() -> Self {
        LowLevelHook::new()
    }
}

thread_local! {
    static CALLBACK: std::cell::RefCell<Option<OnKey>> = const { std::cell::RefCell::new(None) };
    static MODS: std::cell::Cell<Mods> = const { std::cell::Cell::new(Mods { lshift: false, rshift: false, lctrl: false, rctrl: false }) };
}

/// Whether a key is down right now, asked of the keyboard itself (not of this thread's message state).
fn async_down(vk: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

unsafe extern "system" fn hook_proc(code: i32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_SYSKEYDOWN};
    if code >= 0 {
        // SAFETY: for a WH_KEYBOARD_LL hook with code >= 0, lparam points to a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        // (the modifiers are known from the events this hook has seen, including this one: Shift held down while Tab arrives)
        let mods = MODS.with(|m| {
            let mut now = m.get();
            now.observe(info.vkCode, down);
            m.set(now);
            now
        });
        let swallow = CALLBACK.with(|c| c.borrow_mut().as_mut().is_some_and(|f| f(translate(info.vkCode, info.scanCode, info.flags.0, down, mods))));
        if swallow {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

impl KeyFeed for LowLevelHook {
    /// INSTALLS A SYSTEM-WIDE KEYBOARD HOOK. Called by nothing in this track.
    fn start(&mut self, on_key: OnKey) -> anyhow::Result<()> {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL};
        if self.thread.is_some() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<u32>>();
        let thread = std::thread::Builder::new().name("rf-switcher-keys".into()).spawn(move || {
            CALLBACK.with(|c| *c.borrow_mut() = Some(on_key));
            MODS.with(|m| m.set(Mods::seeded(async_down(VK_LSHIFT), async_down(VK_RSHIFT), async_down(VK_LCONTROL), async_down(VK_RCONTROL))));
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) };
            let hook = match hook {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(e.into()));
                    return;
                }
            };
            // the thread's queue exists before the id is handed out: a WM_QUIT posted at once is not lost
            let mut probe = MSG::default();
            unsafe {
                let _ = PeekMessageW(&mut probe, None, 0, 0, PM_NOREMOVE);
            }
            let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
            CALLBACK.with(|c| *c.borrow_mut() = None);
        })?;
        self.thread_id = rx.recv_timeout(std::time::Duration::from_secs(5))??;
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        if let Some(t) = self.thread.take() {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = t.join();
            self.thread_id = 0;
        }
    }

    fn running(&self) -> bool {
        self.thread.is_some()
    }
}

impl Drop for LowLevelHook {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------- a feed for tests

/// A feed that delivers what the test pushes, on the calling thread.
#[derive(Default)]
pub struct FakeFeed {
    on_key: Option<OnKey>,
}

impl FakeFeed {
    /// Delivers one event; the answer is whether the key would be swallowed. False when not started.
    pub fn push(&mut self, k: KeyInput) -> bool {
        self.on_key.as_mut().is_some_and(|f| f(k))
    }
}

impl KeyFeed for FakeFeed {
    fn start(&mut self, on_key: OnKey) -> anyhow::Result<()> {
        self.on_key = Some(on_key);
        Ok(())
    }
    fn stop(&mut self) {
        self.on_key = None;
    }
    fn running(&self) -> bool {
        self.on_key.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::chord::Effect;
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn virtual_keys_map_to_the_chords_keys() {
        assert_eq!(key_of(VK_TAB, 0x0F), Key::Tab);
        assert_eq!(key_of(VK_LMENU, 0x38), Key::Alt);
        assert_eq!(key_of(VK_RMENU, 0x38), Key::Alt);
        assert_eq!(key_of(VK_MENU, 0x38), Key::Alt);
        assert_eq!(key_of(0x41, 0x1E), Key::Other);
        assert_eq!(key_of(VK_ESCAPE, 0x01), Key::Escape);
        assert_eq!(key_of(VK_RETURN, 0x1C), Key::Enter);
        for (vk, k) in [(VK_LEFT, Key::Left), (VK_RIGHT, Key::Right), (VK_UP, Key::Up), (VK_DOWN, Key::Down)] {
            assert_eq!(key_of(vk, 0x4B), k);
        }
    }

    #[test]
    fn the_same_app_key_is_the_key_above_tab_by_its_scan_code_on_any_layout() {
        // US layout: the back quote is VK_OEM_3 (0xC0); on a layout where that key types a letter it has another virtual-key code
        assert_eq!(key_of(0xC0, SCAN_ABOVE_TAB), Key::Grave);
        assert_eq!(key_of(0xDC, SCAN_ABOVE_TAB), Key::Grave, "another layout's code for the same key");
        // and VK_OEM_3 on some other physical key (a typing key of another layout) is not it
        assert_eq!(key_of(0xC0, 0x27), Key::Other);
        // a modifier is never taken for it
        assert_eq!(key_of(VK_LSHIFT, SCAN_ABOVE_TAB), Key::Other);
    }

    #[test]
    fn the_hook_flags_become_alt_and_injected() {
        let k = translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, Mods::default());
        assert!(k.alt && !k.injected && k.down && !k.shift);
        let k = translate(VK_TAB, 0x0F, LLKHF_INJECTED, false, Mods::seeded(true, false, false, false));
        assert!(!k.alt && k.injected && !k.down && k.shift);
    }

    #[test]
    fn shift_is_known_from_the_hooks_own_events_so_shift_tab_is_not_a_plain_tab() {
        // a hook thread that reads no keyboard messages cannot ask GetKeyState: it follows the events it has been given
        let mut mods = Mods::default();
        assert!(!mods.shift());
        mods.observe(VK_LSHIFT, true);
        let k = translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, mods);
        assert!(k.shift, "Shift+Tab arrives as Shift+Tab");
        mods.observe(VK_RSHIFT, true);
        mods.observe(VK_LSHIFT, false);
        assert!(mods.shift(), "the right Shift still holds it");
        mods.observe(VK_RSHIFT, false);
        assert!(!mods.shift());
        assert!(!translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, mods).shift);
        // other keys change nothing
        mods.observe(VK_TAB, true);
        assert_eq!(mods, Mods::default());
        // a Shift already held when the hook starts counts from the first event
        assert!(Mods::seeded(true, false, false, false).shift());
    }

    #[test]
    fn altgr_with_a_letter_is_not_the_chord_and_is_not_swallowed() {
        // AltGr is a right Alt with a Ctrl Windows makes up; on a layout with a character on AltGr + the key above Tab this must
        // type the character, not open the switcher
        let chord = Arc::new(parking_lot::Mutex::new(Chord::new()));
        let mut feed = FakeFeed::default();
        feed.start(chord_callback(chord.clone(), |_| {})).unwrap();
        let mut mods = Mods::default();
        mods.observe(VK_LCONTROL, true); // the made-up Ctrl
        assert!(!feed.push(translate(VK_RMENU, 0x38, LLKHF_ALTDOWN, true, mods)));
        assert!(!feed.push(translate(0xC0, SCAN_ABOVE_TAB, LLKHF_ALTDOWN, true, mods)), "AltGr + the key above Tab types its character");
        assert!(!feed.push(translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, mods)), "Ctrl+Alt+Tab is Windows' own");
        assert!(!chord.lock().is_open());
        // without the Ctrl it is the chord
        assert!(feed.push(translate(0xC0, SCAN_ABOVE_TAB, LLKHF_ALTDOWN, true, Mods::default())));
        assert!(chord.lock().is_open());
    }

    #[test]
    fn a_fake_feed_drives_the_chord_and_reports_what_would_be_swallowed() {
        let chord = Arc::new(parking_lot::Mutex::new(Chord::new()));
        let seen: Arc<Mutex<Vec<Reaction>>> = Default::default();
        let s2 = seen.clone();
        let mut feed = FakeFeed::default();
        assert!(!feed.running());
        feed.start(chord_callback(chord, move |r| s2.lock().unwrap().push(r))).unwrap();
        assert!(feed.running());
        // Alt+Tab, Tab again, release both, release Alt
        let none = Mods::default();
        assert!(feed.push(translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, none)), "the Tab is swallowed");
        assert!(feed.push(translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, none)));
        assert!(feed.push(translate(VK_TAB, 0x0F, 0, false, none)), "its release too");
        assert!(!feed.push(translate(VK_LMENU, 0x38, 0, false, none)), "the Alt release is not");
        let reactions = seen.lock().unwrap().clone();
        let effects: Vec<Option<Effect>> = reactions.iter().map(|r| r.effect).collect();
        assert_eq!(
            effects,
            vec![
                Some(Effect::Open { scope: super::super::chord::Scope::AllWindows, backwards: false }),
                Some(Effect::Step { backwards: false }),
                Some(Effect::Choose)
            ]
        );
        assert!(reactions[2].mask_alt, "the host is told the Alt release needs the mask key");
        feed.stop();
        assert!(!feed.running());
        assert!(!feed.push(translate(VK_TAB, 0x0F, LLKHF_ALTDOWN, true, none)), "a stopped feed swallows nothing");
    }

    #[test]
    fn the_real_hook_is_not_running_unless_started() {
        // (constructed and dropped only: `start` installs a system-wide hook and is called by nothing in this track)
        let h = LowLevelHook::new();
        assert!(!h.running());
    }
}
