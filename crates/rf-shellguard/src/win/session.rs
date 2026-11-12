//! The session side: Safe Mode, the Shift key, the build, and the shell's identity events.
//! NOT RUN by the author of this crate (creating and signalling `ShellReadyEvent` is exercised only through `FakeSession`).
//!
//! `ShellReadyEvent` (plan A2): userinit opens it by that name in the session's namespace, waits for it
//! and then records which process owns the shell window. Explorer creates and signals it today.
//! Whether a manual-reset event is what the consumers expect is unverified (plan section 7).

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Console::{CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler};
use windows::Win32::System::Threading::{CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
use windows::core::{BOOL, PCWSTR};

use super::{signals, wide};
use crate::sys::Session;
use crate::winro;

/// Event names. The test names live in `Local\` and carry "test", so a `--test-shell` run can never touch
/// Explorer's `ShellReadyEvent` or a real guard's escape event.
#[derive(Clone)]
pub struct Names {
    pub shell_ready: String,
    pub escape: String,
}

impl Names {
    pub fn real() -> Names {
        Names { shell_ready: "ShellReadyEvent".into(), escape: "Local\\rustyfiles-shellguard-escape".into() }
    }

    pub fn test() -> Names {
        Names { shell_ready: "Local\\rustyfiles-test-ShellReadyEvent".into(), escape: "Local\\rustyfiles-test-shellguard-escape".into() }
    }
}

/// How long a `--panic` process waits for a running guard to take the request up before it does the fall-back itself.
pub const PANIC_ACK_MS: u64 = 3_000;

pub struct RealSession {
    names: Names,
    ready: Option<isize>,
    panic_ack_ms: u64,
}

impl RealSession {
    pub fn new(names: Names) -> RealSession {
        RealSession { names, ready: None, panic_ack_ms: PANIC_ACK_MS }
    }

    /// For tests: how long to wait for a guard's answer to `--panic`.
    pub fn with_panic_ack_ms(mut self, ms: u64) -> RealSession {
        self.panic_ack_ms = ms;
        self
    }
}

unsafe extern "system" fn on_ctrl(ctrl_type: u32) -> BOOL {
    if ctrl_type == CTRL_LOGOFF_EVENT || ctrl_type == CTRL_SHUTDOWN_EVENT {
        signals::ENDING.store(true, std::sync::atomic::Ordering::SeqCst);
        let h = signals::WAKE.load(std::sync::atomic::Ordering::SeqCst);
        if h != 0 {
            // SAFETY: the handle is the escape event, which stays open for the life of the process.
            unsafe {
                let _ = SetEvent(HANDLE(h as *mut _));
            }
        }
        return BOOL(1);
    }
    BOOL(0)
}

/// Makes the guard notice log off and shut down (a process with no window gets them as console control events).
pub fn install_end_session_handler() {
    // SAFETY: `on_ctrl` is a plain function that only sets an atomic and an event.
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
}

impl Session for RealSession {
    fn safe_mode(&mut self) -> bool {
        winro::safe_mode()
    }

    fn shift_held(&mut self) -> bool {
        winro::shift_held()
    }

    fn build(&mut self) -> Option<String> {
        winro::build()
    }

    fn create_shell_ready_event(&mut self) -> Result<(), String> {
        if self.ready.is_some() {
            return Ok(());
        }
        let name = wide(&self.names.shell_ready);
        // SAFETY: `name` is NUL-terminated. Manual reset, not signalled.
        let h = unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) }.map_err(|e| e.to_string())?;
        // SAFETY: reads the thread's last error right after the call.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // Someone else (a guard that did not exit, or Explorer) made it first; we share it and may signal it, which is harmless.
        }
        self.ready = Some(h.0 as isize);
        Ok(())
    }

    fn signal_shell_ready(&mut self) -> Result<(), String> {
        let Some(h) = self.ready else { return Err("ShellReadyEvent was never created".into()) };
        // SAFETY: `h` is the event created above.
        unsafe { SetEvent(HANDLE(h as *mut _)) }.map_err(|e| e.to_string())
    }

    fn release_identity(&mut self) {
        if let Some(h) = self.ready.take() {
            // SAFETY: `h` is the event created above, closed once.
            unsafe {
                let _ = CloseHandle(HANDLE(h as *mut _));
            }
        }
    }

    fn signal_running_guard_panic(&mut self) -> bool {
        let name = wide(&self.names.escape);
        // SAFETY: `name` is NUL-terminated.
        match unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) } {
            Ok(h) => {
                // SAFETY: `h` was just opened.
                let mut taken = unsafe { SetEvent(h) }.is_ok();
                if taken {
                    // The event is manual-reset and the guard resets it when it takes the request up. An event that
                    // stays set means the guard is hung (or about to die): then this process must not leave
                    // believing it was handed over, because nothing would ever put Explorer back.
                    let started = Instant::now();
                    // SAFETY: `h` is open with the synchronize right.
                    while unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0 {
                        if started.elapsed() >= Duration::from_millis(self.panic_ack_ms) {
                            taken = false;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
                // SAFETY: closed once.
                unsafe {
                    let _ = CloseHandle(h);
                }
                taken
            }
            Err(_) => false,
        }
    }

    fn session_ending(&mut self) -> bool {
        // The console control handler is not called in a process that has loaded user32 (this one has), so the
        // session's own "shutting down" flag is the reliable sign.
        // SAFETY: a plain metric query.
        signals::ending() || unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
    }
}
