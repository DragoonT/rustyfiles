//! The events that tell the engine the foreground window changed, so that it does not have to look.
//!
//! WHY. What a glass window is owed depends on whether it is in front (`rest.rs`). The engine reads that on every turn it makes anyway,
//! but a window whose glass is frozen, or glass that is seen with nothing in front, lets the engine sleep for a long time; the turn that
//! would notice "the user clicked this window" must come when it happens, not when a clock says so. The system tells: a WinEvent for a
//! change of the foreground window. The hook is out of context (nothing is injected into other processes, the callback runs on the thread
//! below), it costs one tiny message per change of the foreground window and nothing in between, and the callback only puts a command
//! in the engine's channel. The pointer coming over a window is the other event; it comes from the app's own window (`glass::pointer_over`).
//!
//! One thread per process, started by the engine with the first glass window and never stopped (it blocks in `GetMessageW`, no timer).
//! If the hook cannot be set the engine keeps looking on its own short clock, as before (`running` is false).

use std::sync::OnceLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, TranslateMessage, WINEVENT_OUTOFCONTEXT};

use super::Cmd;

/// Whether the hook is set (`None`: not started yet).
static HOOKED: OnceLock<bool> = OnceLock::new();

unsafe extern "system" fn on_foreground(_hook: HWINEVENTHOOK, _event: u32, _hwnd: HWND, _id_object: i32, _id_child: i32, _thread: u32, _time: u32) {
    // runs on the watch thread: only wake the engine, which looks at who is in front itself
    super::send_if_running(Cmd::CareChanged);
}

/// Starts the watch thread once and says whether the hook is set. Not in the unit tests: they have no engine to wake.
pub(super) fn start() -> bool {
    if cfg!(test) {
        return false;
    }
    *HOOKED.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let spawned = std::thread::Builder::new().name("rf-glass-watch".into()).spawn(move || {
            let hook = unsafe { SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT) };
            let _ = tx.send(!hook.is_invalid());
            if hook.is_invalid() {
                return;
            }
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let _ = UnhookWinEvent(hook);
            }
        });
        let ok = spawned.is_ok() && rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or(false);
        if !ok {
            log::info!("glass: the foreground window cannot be watched by an event; the engine keeps looking on its own clock");
        }
        ok
    })
}

/// Whether the engine can rely on being woken when the foreground window changes.
pub(super) fn running() -> bool {
    HOOKED.get().copied().unwrap_or(false)
}
