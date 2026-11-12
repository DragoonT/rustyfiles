//! Text output for `--dry-run`, `--check` and error messages. The program is built for the Windows
//! subsystem (no console window when Winlogon starts it), so a console it was started from has to be
//! attached by hand; when output is redirected the standard handle is already valid and is used as is.

use std::io::Write;

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};

fn std_out_is_usable() -> bool {
    // SAFETY: a plain query.
    match unsafe { GetStdHandle(STD_OUTPUT_HANDLE) } {
        Ok(h) => !h.is_invalid() && !h.0.is_null(),
        Err(_) => false,
    }
}

/// A writer for text meant for the person who started the program.
pub fn out() -> Box<dyn Write> {
    if std_out_is_usable() {
        return Box::new(std::io::stdout());
    }
    // SAFETY: attaches to the parent's console if it has one; creates no window.
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok();
    if attached && let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        return Box::new(f);
    }
    Box::new(std::io::sink())
}
