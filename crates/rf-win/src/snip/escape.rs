//! Escape for the snip overlay, taken away from the program under it.
//!
//! The overlay's windows never take the keyboard (see `place.rs`), so the program the user was in still has it. Reading Escape from the
//! keyboard's state only OBSERVES the key: the same Escape would also cancel the dialog, abandon the rename or leave the full-screen video
//! underneath. So while an overlay is up this holds a low-level keyboard hook that does two things with ONE key: it reports a fresh press
//! of a plain Escape to the snip, and it swallows that press (and its repeats and its release) so that nobody else sees it. Every other
//! key, and Escape with Ctrl, Alt or a Windows key down, passes untouched; nothing is recorded.
//!
//! The hook has a thread of its own (a hook is called on the thread that installed it, and a busy interface thread would hold up the
//! keyboard of the whole system), exists only while a [`EscapeGrab`] is alive, and goes with it. An Escape that is already down when the
//! overlay appears (the key that started something else, still held) is not the snip's: it passes, with its repeats, until it has been
//! seen up once.
//!
//! The decision is a small pure state machine ([`EscState`]), tested here; the hook only feeds it. NOT run against a real keyboard in
//! this increment (no keystrokes may be sent): the lead's live test covers it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_RWIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// What the hook does with one Escape event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// The event is taken: no other program sees it.
    pub swallow: bool,
    /// A fresh press the snip should act on (reported once per press, not for repeats).
    pub press: bool,
}

const PASS: Verdict = Verdict { swallow: false, press: false };

/// The Escape key as the hook sees it, pure.
#[derive(Clone, Copy, Debug)]
pub struct EscState {
    /// Escape has been seen up since the grab began (or was up when it began).
    seen_up: bool,
    /// The press now down was taken by us, so its release is ours too.
    down_taken: bool,
}

impl EscState {
    /// `down_now`: whether Escape is down at the moment the grab begins.
    pub fn new(down_now: bool) -> EscState {
        EscState { seen_up: !down_now, down_taken: false }
    }

    /// One Escape event. `plain`: no Ctrl, Alt or Windows key is down (Escape with one of them is the system's: Ctrl+Esc, Alt+Esc).
    pub fn on_key(&mut self, down: bool, plain: bool) -> Verdict {
        if !plain {
            return PASS;
        }
        if down {
            if !self.seen_up {
                // the continuation of a press that began before the overlay: not ours
                return PASS;
            }
            let first = !self.down_taken;
            self.down_taken = true;
            Verdict { swallow: true, press: first }
        } else {
            self.seen_up = true;
            // (a release whose press we did not take goes through, or the program that has the key down would never see it up)
            let ours = std::mem::take(&mut self.down_taken);
            Verdict { swallow: ours, press: false }
        }
    }
}

static STATE: Mutex<Option<EscState>> = Mutex::new(None);
static PRESSED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the hook is installed and running (when it is not, the snip falls back to looking at the keyboard's state).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Whether a fresh Escape press came since the last call (and forgets it).
pub fn take_pressed() -> bool {
    PRESSED.swap(false, Ordering::AcqRel)
}

/// What a hook call does, from the message and the key's data, over `state`. Split from the system call so that a test can feed it.
fn handle(state: &mut EscState, msg: u32, vk: u32, flags: u32, plain_mods: bool) -> Verdict {
    if vk != u32::from(VK_ESCAPE.0) {
        return PASS;
    }
    let down = match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return PASS,
    };
    let plain = plain_mods && flags & LLKHF_ALTDOWN.0 == 0;
    state.on_key(down, plain)
}

fn mod_down(vk: u16) -> bool {
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    state < 0
}

/// What the hook procedure decides for one call, from the system's arguments: `true` when the event is taken. Split from the call out
/// to the next hook so that a test can drive it without a hook, a key or a call into the system's chain.
///
/// # Safety
/// `lp` is zero or points at a `KBDLLHOOKSTRUCT` (what a low-level keyboard hook is given).
unsafe fn taken(code: i32, wp: WPARAM, lp: LPARAM, plain_mods: impl FnOnce() -> bool) -> bool {
    if code < 0 || lp.0 == 0 {
        return false;
    }
    // SAFETY: by the contract above
    let kb = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
    if kb.vkCode != u32::from(VK_ESCAPE.0) {
        return false;
    }
    let plain_mods = plain_mods();
    // (the lock is held for the decision only; a poisoned lock is used as it is, so that a panic elsewhere cannot stop the keyboard)
    let verdict = {
        let mut g = STATE.lock().unwrap_or_else(|p| p.into_inner());
        g.as_mut().map(|s| handle(s, wp.0 as u32, kb.vkCode, kb.flags.0, plain_mods))
    };
    match verdict {
        Some(v) => {
            if v.press {
                PRESSED.store(true, Ordering::Release);
            }
            v.swallow
        }
        None => false,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: `lp` is what the system gave a low-level keyboard hook
    if unsafe { taken(code, wp, lp, || !(mod_down(VK_CONTROL.0) || mod_down(VK_LWIN.0) || mod_down(VK_RWIN.0))) } {
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// The hook's thread. Dropping it takes the hook off.
pub struct EscapeGrab {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl EscapeGrab {
    /// INSTALLS A SYSTEM-WIDE KEYBOARD HOOK for the one key. `None` when it could not be installed (the snip then looks at the keyboard's
    /// state, and Escape also reaches the program underneath) or when one is already running.
    pub fn start() -> Option<EscapeGrab> {
        if ACTIVE.swap(true, Ordering::AcqRel) {
            return None;
        }
        *STATE.lock().unwrap_or_else(|p| p.into_inner()) = Some(EscState::new(mod_down(VK_ESCAPE.0)));
        PRESSED.store(false, Ordering::Release);
        let (tx, rx) = std::sync::mpsc::channel::<Option<u32>>();
        let spawned = std::thread::Builder::new().name("rf-snip-esc".into()).spawn(move || {
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) };
            let Ok(hook) = hook else {
                let _ = tx.send(None);
                return;
            };
            // the thread's queue exists before its id is handed out: a WM_QUIT posted at once is not lost
            let mut probe = MSG::default();
            unsafe {
                let _ = PeekMessageW(&mut probe, None, 0, 0, PM_NOREMOVE);
            }
            let _ = tx.send(Some(unsafe { GetCurrentThreadId() }));
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
        });
        let reset = || {
            *STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
            ACTIVE.store(false, Ordering::Release);
        };
        let Ok(thread) = spawned else {
            reset();
            return None;
        };
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Some(thread_id)) => Some(EscapeGrab { thread_id, thread: Some(thread) }),
            _ => {
                // (a thread that comes up late finds nobody to talk to and ends with its hook when the queue is told to quit)
                reset();
                None
            }
        }
    }
}

impl Drop for EscapeGrab {
    fn drop(&mut self) {
        // the key stops being taken first, then the thread ends
        *STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        PRESSED.store(false, Ordering::Release);
        ACTIVE.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_escape_is_reported_once_and_taken_with_its_repeats_and_its_release() {
        let mut s = EscState::new(false);
        assert_eq!(s.on_key(true, true), Verdict { swallow: true, press: true }, "taken: the program underneath must not see it");
        assert_eq!(s.on_key(true, true), Verdict { swallow: true, press: false }, "a repeat is taken and is not another press");
        assert_eq!(s.on_key(false, true), Verdict { swallow: true, press: false }, "its release is taken too, or the program sees an Escape up with no down");
        assert_eq!(s.on_key(true, true), Verdict { swallow: true, press: true }, "and the next press counts again");
    }

    #[test]
    fn an_escape_already_down_when_the_grab_begins_is_not_the_snips_and_goes_through() {
        let mut s = EscState::new(true);
        assert_eq!(s.on_key(true, true), PASS, "the repeats of the older press go to the program that has it down");
        assert_eq!(s.on_key(false, true), PASS, "its release goes through, or that program would never see it up");
        assert_eq!(s.on_key(true, true), Verdict { swallow: true, press: true }, "a press after that is ours");
    }

    #[test]
    fn escape_with_ctrl_alt_or_a_windows_key_is_the_systems() {
        let mut s = EscState::new(false);
        assert_eq!(s.on_key(true, false), PASS);
        assert_eq!(s.on_key(false, false), PASS);
        // and it leaves the state alone: the next plain press is still fresh
        assert_eq!(s.on_key(true, true), Verdict { swallow: true, press: true });
    }

    #[test]
    fn a_release_whose_press_was_not_taken_is_not_taken() {
        let mut s = EscState::new(false);
        // Ctrl+Esc went down (passed), Ctrl was let go, then Escape came up plain
        assert_eq!(s.on_key(true, false), PASS);
        assert_eq!(s.on_key(false, true), PASS);
    }

    #[test]
    fn the_hook_decides_only_for_the_escape_key_and_only_for_key_messages() {
        let esc = u32::from(VK_ESCAPE.0);
        let mut s = EscState::new(false);
        // another key, in every message: untouched
        for vk in [0x41u32, 0x0D, 0x2E, 0x1B + 1] {
            assert_eq!(handle(&mut s, WM_KEYDOWN, vk, 0, true), PASS, "vk {vk:#x}");
            assert_eq!(handle(&mut s, WM_KEYUP, vk, 0, true), PASS);
        }
        // a message that is not a key message
        assert_eq!(handle(&mut s, 0x0200, esc, 0, true), PASS);
        // Escape: taken
        assert!(handle(&mut s, WM_KEYDOWN, esc, 0, true).swallow);
        assert!(handle(&mut s, WM_KEYUP, esc, 0, true).swallow);
        // Alt+Esc arrives as a system key with the Alt flag: the system's
        assert_eq!(handle(&mut s, WM_SYSKEYDOWN, esc, LLKHF_ALTDOWN.0, true), PASS);
        // a Ctrl or Windows key down
        assert_eq!(handle(&mut s, WM_KEYDOWN, esc, 0, false), PASS);
    }

    #[test]
    fn the_hook_decision_takes_through_the_shared_state_and_lets_everything_pass_without_a_grab() {
        // Drives the decision with a KBDLLHOOKSTRUCT of our own. No hook is installed, no key is sent and nothing is passed on to the
        // system's chain. (The shared state is global, so tests that touch it run one at a time.)
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let kb = KBDLLHOOKSTRUCT { vkCode: u32::from(VK_ESCAPE.0), ..Default::default() };
        let lp = LPARAM(&kb as *const KBDLLHOOKSTRUCT as isize);
        let down = WPARAM(WM_KEYDOWN as usize);
        let up = WPARAM(WM_KEYUP as usize);
        // no grab: nothing is taken
        *STATE.lock().unwrap() = None;
        assert!(!unsafe { taken(0, down, lp, || true) });
        // a grab that began with Escape up: the press is taken and reported once, and so is its release
        *STATE.lock().unwrap() = Some(EscState::new(false));
        PRESSED.store(false, Ordering::SeqCst);
        assert!(unsafe { taken(0, down, lp, || true) }, "swallowed");
        assert!(take_pressed());
        assert!(!take_pressed(), "reported once");
        assert!(unsafe { taken(0, up, lp, || true) });
        // a code below zero is the system saying "do not process"; a null pointer is nothing
        assert!(!unsafe { taken(-1, down, lp, || true) });
        assert!(!unsafe { taken(0, down, LPARAM(0), || true) });
        // another key is never taken, grab or no grab
        let other = KBDLLHOOKSTRUCT { vkCode: 0x41, ..Default::default() };
        assert!(!unsafe { taken(0, down, LPARAM(&other as *const KBDLLHOOKSTRUCT as isize), || true) });
        // a Ctrl or Windows key down: the system's Escape
        assert!(!unsafe { taken(0, down, lp, || false) });
        *STATE.lock().unwrap() = None;
    }
}
