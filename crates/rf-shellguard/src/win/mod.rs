//! The real implementations of the `sys` traits. Nothing in here is reached by the tests or by
//! `--dry-run`; only the binary's real modes (`main.rs`) build these. Every write to the system
//! the guard ever makes is in this folder:
//!
//! * `registry`: deletes the Winlogon Shell value of the current user (never writes it).
//! * `procs`: starts the host and `explorer.exe`, kills the host.
//! * `session`: creates and signals `ShellReadyEvent`.
//! * `hotkey`: the escape chord (a hotkey and a keyboard hook on a thread of their own).
//! * `taskbar` (re-exports `tbwin`, the only file that hides or shows a window of Explorer's), `workarea`: undo what a dead host left changed.
//! * `link`: opens and sets the two named events of the dock host's beside-guard.
//! * `ui`: the one message box.

pub mod console;
pub mod hotkey;
pub mod link;
pub mod procs;
pub mod registry;
pub mod session;
pub mod signals;
pub mod taskbar;
pub mod tbwin;
pub mod ui;
pub mod workarea;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::machine::Millis;
use crate::sys::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> Millis {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as Millis).unwrap_or(0)
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
