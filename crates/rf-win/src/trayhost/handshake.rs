//! The start handshake between the caller's thread (`win::spawn`, which waits at most 5 s for the host to say it is up) and the host
//! thread. Pure: two atomics and the rule for who closes the window when the caller has given up.
//!
//! THE PROBLEM. `spawn` gives up after 5 s and returns an error, and the app then believes the host does not exist and keeps no
//! handle to stop it. If the host thread was merely slow (a stalled machine, a virus scanner hooking `CreateWindow`), it would
//! finish its start later and leave a `Shell_TrayWnd` that nothing can close. Both sides therefore meet here:
//!
//! * the host thread, once its window exists, calls [`Handshake::published`]; a `true` answer means the caller already gave up and
//!   the host must close its own window at once;
//! * the caller, when it gives up, calls [`Handshake::give_up`]; the window handle it returns (non-zero if the host had published
//!   one) is closed by the caller (`WM_CLOSE` posted to it).
//!
//! Each side stores its fact and THEN reads the other's (sequentially consistent), so at least one of them sees both: the window is
//! never left behind. If both see it the window is asked to close twice, which is harmless.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

#[derive(Default)]
pub struct Handshake {
    hwnd: AtomicIsize,
    gave_up: AtomicBool,
}

impl Handshake {
    pub fn new() -> Handshake {
        Handshake::default()
    }

    /// The host's window as published (0: none). `TrayHost::stop` posts its close request here.
    pub fn hwnd(&self) -> isize {
        self.hwnd.load(Ordering::SeqCst)
    }

    /// Host thread: has the caller given up? Asked before the window is made, so that a slow start that nobody waits for makes none.
    pub fn gave_up(&self) -> bool {
        self.gave_up.load(Ordering::SeqCst)
    }

    /// Host thread: the window exists. True if the caller gave up meanwhile: the host closes the window itself.
    pub fn published(&self, hwnd: isize) -> bool {
        self.hwnd.store(hwnd, Ordering::SeqCst);
        self.gave_up.load(Ordering::SeqCst)
    }

    /// Caller: stops waiting. Returns the window to close, if the host had published one.
    pub fn give_up(&self) -> isize {
        self.gave_up.store(true, Ordering::SeqCst);
        self.hwnd.load(Ordering::SeqCst)
    }

    /// Host thread: the window is gone.
    pub fn cleared(&self) {
        self.hwnd.store(0, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_host_that_is_published_before_the_caller_gives_up_is_closed_by_the_caller() {
        let h = Handshake::new();
        assert!(!h.published(0x40));
        assert_eq!(h.give_up(), 0x40, "the caller gets the window to close");
    }

    #[test]
    fn a_host_that_publishes_after_the_caller_gave_up_closes_itself() {
        let h = Handshake::new();
        assert_eq!(h.give_up(), 0, "nothing published yet: nothing for the caller to close");
        assert!(h.published(0x40), "the host is told at once");
    }

    #[test]
    fn a_host_that_has_not_made_its_window_when_the_caller_gives_up_makes_none() {
        let h = Handshake::new();
        h.give_up();
        assert!(h.gave_up());
    }

    #[test]
    fn a_caller_that_does_not_give_up_leaves_the_host_alone() {
        let h = Handshake::new();
        assert!(!h.gave_up());
        assert!(!h.published(0x40));
        assert_eq!(h.hwnd(), 0x40);
        h.cleared();
        assert_eq!(h.hwnd(), 0);
    }

    #[test]
    fn in_every_interleaving_somebody_closes_the_window() {
        // two real threads race 2000 times; the window is never left with nobody to close it
        for _ in 0..2000 {
            let h = Arc::new(Handshake::new());
            let h2 = h.clone();
            let host = std::thread::spawn(move || h2.published(0x77));
            let caller_saw = h.give_up();
            let host_saw = host.join().unwrap();
            assert!(host_saw || caller_saw == 0x77, "the window would have been left behind");
        }
    }
}
