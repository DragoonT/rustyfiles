//! The keyboard feed of the chord: the thing that sees Alt+Tab before the program in front does and tells the switcher.
//!
//! The trait [`KeyFeed`] is what the shell host uses; [`LowLevelHook`] is the real implementation (a `WH_KEYBOARD_LL` hook on a
//! thread of its own). It is the ONE keyboard hook of the product: the shell host's keyboard layer (`rf_win::winkey`) starts it,
//! only while one of the settings `shell_win_key` and `shell_own_switcher` is on, and stops it when both are off. Tests and probes
//! use [`FakeFeed`]; the real hook is exercised only through [`translate`] and [`RawTracker`] (the pure parts). A feed delivers
//! either the chord's [`KeyInput`]s ([`KeyFeed::start`]) or every event as it came ([`KeyFeed::start_raw`], which the Windows-key
//! machine needs: it must see the Windows keys, which the chord has no name for).
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

/// Like [`OnKey`], for the event as the system gave it (see [`RawKey`]).
pub type OnRaw = Box<dyn FnMut(&RawKey) -> bool + Send>;

/// Something that delivers key events to a callback until it is stopped.
pub trait KeyFeed {
    /// The chord's view of the events. The default makes it from [`KeyFeed::start_raw`].
    fn start(&mut self, mut on_key: OnKey) -> anyhow::Result<()> {
        self.start_raw(Box::new(move |k| on_key(translate(k.vk, k.scan, k.flags, k.down, k.mods))))
    }
    /// Every event, as it came, with the Shift and Ctrl state the feed has kept.
    fn start_raw(&mut self, on_raw: OnRaw) -> anyhow::Result<()>;
    fn stop(&mut self);
    /// Whether the feed is delivering: false after `stop`, and false when the thread behind a real hook has ended by itself.
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
/// so that its release is seen. That holds for PRESSES only: a release is always the key it is, because the chord finds the release
/// of a swallowed press by the key's own identity (Ctrl pressed between a swallowed Tab and its release must not turn that release
/// into "another key", which would pass it and leave the Tab marked as swallowed for the next, plain Tab).
pub fn translate(vk: u32, scan: u32, flags: u32, down: bool, mods: Mods) -> KeyInput {
    let key = match key_of(vk, scan) {
        Key::Alt => Key::Alt,
        _ if down && mods.ctrl() => Key::Other,
        k => k,
    };
    KeyInput { key, down, shift: mods.shift(), alt: flags & LLKHF_ALTDOWN != 0, injected: flags & LLKHF_INJECTED != 0 }
}

// ---------------------------------------------------------------- the event as it came

/// One hook event: the fields of the system's `KBDLLHOOKSTRUCT` that are used, `down` from the message, and the Shift and Ctrl
/// state AFTER this event. Plain numbers, `Copy`: building one allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawKey {
    pub vk: u32,
    pub scan: u32,
    pub flags: u32,
    pub down: bool,
    pub mods: Mods,
}

impl RawKey {
    /// The event was sent by a program (`LLKHF_INJECTED`), not typed.
    pub const fn injected(&self) -> bool {
        self.flags & LLKHF_INJECTED != 0
    }
}

/// Keeps the Shift and Ctrl state from the events a hook sees and turns each event into a [`RawKey`]. The real hook and
/// [`FakeFeed`] use the same one, so a test of the layer sees what the hook thread would.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawTracker {
    mods: Mods,
}

impl RawTracker {
    pub const fn seeded(mods: Mods) -> RawTracker {
        RawTracker { mods }
    }

    pub fn observe(&mut self, vk: u32, scan: u32, flags: u32, down: bool) -> RawKey {
        self.mods.observe(vk, down);
        RawKey { vk, scan, flags, down, mods: self.mods }
    }
}

// ---------------------------------------------------------------- the real hook (installed only by `rf_win::winkey`)

pub struct LowLevelHook {
    thread: Option<std::thread::JoinHandle<()>>,
    thread_id: u32,
}

/// The meeting point of the starter and the hook thread when the starter may give up waiting. Without it a hook thread that was merely
/// slow (heavy load) would install its hook AFTER `start_raw` had returned an error: nobody holds the thread's handle or id, so nobody
/// could stop it, and the next reinstallation would add a second hook. Each side does its store first and then looks at the other's
/// (`SeqCst`), so at least one of them sees the other: either the starter finds the id and ends the thread, or the thread finds the
/// flag and ends itself.
#[derive(Default)]
struct Handshake {
    id: std::sync::atomic::AtomicU32,
    abandoned: std::sync::atomic::AtomicBool,
}

impl Handshake {
    /// The hook thread has its hook and its queue. False: the starter gave up, the thread must remove its hook and end.
    fn publish(&self, id: u32) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        self.id.store(id, SeqCst);
        !self.abandoned.load(SeqCst)
    }

    /// The starter gives up waiting. Some(id): the thread is already up and must be told to quit; None: it will see the flag.
    fn abandon(&self) -> Option<u32> {
        use std::sync::atomic::Ordering::SeqCst;
        self.abandoned.store(true, SeqCst);
        let id = self.id.load(SeqCst);
        (id != 0).then_some(id)
    }
}

/// Asks a hook thread to end (its message loop leaves on `WM_QUIT`, removes the hook and returns).
fn post_quit(thread_id: u32) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
    unsafe {
        let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
    }
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
    static CALLBACK: std::cell::RefCell<Option<OnRaw>> = const { std::cell::RefCell::new(None) };
    static TRACKER: std::cell::Cell<RawTracker> = const { std::cell::Cell::new(RawTracker { mods: Mods { lshift: false, rshift: false, lctrl: false, rctrl: false } }) };
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
        let raw = TRACKER.with(|t| {
            let mut tracker = t.get();
            let raw = tracker.observe(info.vkCode, info.scanCode, info.flags.0, down);
            t.set(tracker);
            raw
        });
        // A callback that sends a key makes the system call this function again, on this thread, while the callback is still running
        // (and so still borrowed): that event is our own, and is passed on untouched. A callback that panics must not unwind into the
        // system: the key passes and the hook stays.
        let swallow = CALLBACK.with(|c| match c.try_borrow_mut() {
            Ok(mut slot) => slot.as_mut().is_some_and(|f| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&raw))).unwrap_or(false)),
            Err(_) => false,
        });
        if swallow {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

impl KeyFeed for LowLevelHook {
    /// INSTALLS A SYSTEM-WIDE KEYBOARD HOOK. Called only by `rf_win::winkey::KeyService`.
    fn start_raw(&mut self, on_raw: OnRaw) -> anyhow::Result<()> {
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL};
        if self.thread.is_some() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<u32>>();
        let handshake = Arc::new(Handshake::default());
        let for_thread = handshake.clone();
        let thread = std::thread::Builder::new().name("rf-keys-hook".into()).spawn(move || {
            CALLBACK.with(|c| *c.borrow_mut() = Some(on_raw));
            TRACKER.with(|m| m.set(RawTracker::seeded(Mods::seeded(async_down(VK_LSHIFT), async_down(VK_RSHIFT), async_down(VK_LCONTROL), async_down(VK_RCONTROL)))));
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
            let id = unsafe { GetCurrentThreadId() };
            let _ = tx.send(Ok(id));
            if !for_thread.publish(id) {
                // the starter gave up waiting and returned an error: nobody will stop this thread, so it removes its own hook
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
                CALLBACK.with(|c| *c.borrow_mut() = None);
                return;
            }
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
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(id)) => {
                self.thread_id = id;
                self.thread = Some(thread);
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(e) => {
                // (a thread that comes up later ends itself; one that is already up is told to quit here)
                if let Some(id) = handshake.abandon() {
                    post_quit(id);
                }
                Err(e.into())
            }
        }
    }

    fn stop(&mut self) {
        if let Some(t) = self.thread.take() {
            post_quit(self.thread_id);
            let _ = t.join();
            self.thread_id = 0;
        }
    }

    fn running(&self) -> bool {
        // (a thread that has ended - the hook could not be installed, or the thread died - is not a running hook)
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
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
    on_raw: Option<OnRaw>,
    tracker: RawTracker,
}

impl FakeFeed {
    /// Delivers one event; the answer is whether the key would be swallowed. False when not started.
    pub fn push(&mut self, k: KeyInput) -> bool {
        self.on_key.as_mut().is_some_and(|f| f(k))
    }

    /// Delivers one event as the system would give it (Shift and Ctrl are kept by the feed, as the real hook does). The answer is
    /// whether the key would be swallowed. False when not started.
    pub fn push_raw(&mut self, vk: u32, scan: u32, flags: u32, down: bool) -> bool {
        let raw = self.tracker.observe(vk, scan, flags, down);
        self.on_raw.as_mut().is_some_and(|f| f(&raw))
    }
}

impl KeyFeed for FakeFeed {
    fn start(&mut self, on_key: OnKey) -> anyhow::Result<()> {
        self.on_key = Some(on_key);
        Ok(())
    }
    fn start_raw(&mut self, on_raw: OnRaw) -> anyhow::Result<()> {
        self.on_raw = Some(on_raw);
        Ok(())
    }
    fn stop(&mut self) {
        self.on_key = None;
        self.on_raw = None;
    }
    fn running(&self) -> bool {
        self.on_key.is_some() || self.on_raw.is_some()
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
    fn a_raw_feed_sees_every_event_and_keeps_shift_and_ctrl_like_the_real_hook() {
        let seen: Arc<Mutex<Vec<RawKey>>> = Default::default();
        let s2 = seen.clone();
        let mut feed = FakeFeed::default();
        feed.start_raw(Box::new(move |k| {
            s2.lock().unwrap().push(*k);
            false
        }))
        .unwrap();
        assert!(feed.running());
        feed.push_raw(VK_LSHIFT, 0x2A, 0, true);
        feed.push_raw(0x5B, 0x5B, 0, true); // a key the chord has no name for
        feed.push_raw(VK_TAB, 0x0F, LLKHF_INJECTED, true);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3);
        assert!(seen[0].mods.shift() && seen[1].mods.shift(), "Shift is kept from the events");
        assert_eq!(seen[1].vk, 0x5B);
        assert!(seen[2].injected() && !seen[1].injected());
        feed.stop();
        assert!(!feed.running());
        assert!(!feed.push_raw(VK_TAB, 0x0F, 0, true));
    }

    #[test]
    fn the_chord_view_is_made_from_the_raw_view_by_the_default_start() {
        // a feed that only knows start_raw still serves the chord: the default `start` translates each event
        struct RawOnly(Option<OnRaw>);
        impl KeyFeed for RawOnly {
            fn start_raw(&mut self, on_raw: OnRaw) -> anyhow::Result<()> {
                self.0 = Some(on_raw);
                Ok(())
            }
            fn stop(&mut self) {
                self.0 = None;
            }
            fn running(&self) -> bool {
                self.0.is_some()
            }
        }
        let chord = Arc::new(parking_lot::Mutex::new(Chord::new()));
        let mut feed = RawOnly(None);
        feed.start(chord_callback(chord.clone(), |_| {})).unwrap();
        let mut tracker = RawTracker::default();
        let raw = tracker.observe(VK_TAB, 0x0F, LLKHF_ALTDOWN, true);
        assert!((feed.0.as_mut().unwrap())(&raw), "Alt+Tab reaches the chord and is swallowed");
        assert!(chord.lock().is_open());
    }

    #[test]
    fn a_hook_thread_that_comes_up_after_the_starter_gave_up_ends_itself() {
        let h = Handshake::default();
        assert_eq!(h.abandon(), None, "the thread is not up yet: the starter has nobody to tell");
        assert!(!h.publish(77), "it comes up late, sees the flag, and must remove its hook and end");
    }

    #[test]
    fn a_hook_thread_that_is_already_up_when_the_starter_gives_up_is_told_to_quit() {
        let h = Handshake::default();
        assert!(h.publish(77), "published in time: it goes on");
        assert_eq!(h.abandon(), Some(77), "the starter gives up after all and quits it by its id");
    }

    #[test]
    fn the_real_hook_is_not_running_unless_started() {
        // (constructed and dropped only: `start` installs a system-wide hook and is called by nothing in this track)
        let h = LowLevelHook::new();
        assert!(!h.running());
    }
}
