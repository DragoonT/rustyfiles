//! Process-wide flags that a callback (a hook, a console control handler) can set without locking.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// The auto-reset event the escape chord (and a `--panic` process) signals. 0 until the hotkey is armed.
pub static WAKE: AtomicIsize = AtomicIsize::new(0);
/// The session is ending (log off, shut down): the guard just exits.
pub static ENDING: AtomicBool = AtomicBool::new(false);

pub fn ending() -> bool {
    ENDING.load(Ordering::SeqCst)
}
