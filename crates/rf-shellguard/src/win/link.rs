//! The guard's side of the two named events the dock host created (shell plan E1). NOT RUN by the author of this crate: the pair is
//! exercised only through `FakeLink`; `tests/real_link.rs` opens events that the test itself creates (names under `Local\`, with
//! the test's process id in them), which touches nothing of Windows'.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_ACCESS_RIGHTS, SYNCHRONIZATION_SYNCHRONIZE, SetEvent};
use windows::core::PCWSTR;

use super::wide;
use crate::sys::HostLink;

pub struct RealLink {
    ready_name: String,
    done_name: String,
    ready: Option<isize>,
    done: Option<isize>,
}

impl RealLink {
    pub fn new(ready_name: &str, done_name: &str) -> RealLink {
        RealLink { ready_name: ready_name.to_string(), done_name: done_name.to_string(), ready: None, done: None }
    }
}

fn open(name: &str, rights: SYNCHRONIZATION_ACCESS_RIGHTS) -> Result<isize, String> {
    let w = wide(name);
    // SAFETY: `w` is NUL-terminated and outlives the call.
    unsafe { OpenEventW(rights, false, PCWSTR(w.as_ptr())) }.map(|h| h.0 as isize).map_err(|e| format!("{name}: {e}"))
}

impl HostLink for RealLink {
    fn arm(&mut self) -> Result<(), String> {
        // The ready event is only set; the done event is only waited on.
        self.ready = Some(open(&self.ready_name, EVENT_MODIFY_STATE)?);
        self.done = Some(open(&self.done_name, SYNCHRONIZATION_SYNCHRONIZE)?);
        Ok(())
    }

    fn done_waitable(&self) -> isize {
        self.done.unwrap_or(0)
    }

    fn confirm_ready(&mut self) -> Result<(), String> {
        let Some(h) = self.ready else { return Err("not armed".into()) };
        // SAFETY: the handle was opened by `arm` and is closed in `drop`.
        unsafe { SetEvent(HANDLE(h as *mut _)) }.map_err(|e| e.to_string())
    }
}

impl Drop for RealLink {
    fn drop(&mut self) {
        for h in [self.ready.take(), self.done.take()].into_iter().flatten() {
            // SAFETY: opened in `arm`, closed once.
            unsafe {
                let _ = CloseHandle(HANDLE(h as *mut _));
            }
        }
    }
}
