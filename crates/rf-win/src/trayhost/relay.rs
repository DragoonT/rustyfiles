//! The relay: finds Explorer's own `Shell_TrayWnd`, keeps it cached, checks it before each send, looks again when it is gone
//! (Explorer restarted) and passes messages on. The decisions (which window is the real one, when a forward is retried) are the pure
//! ones of `tray_wire::forward`; this file adds the cache and the order of the steps, over two small traits so that it is tested with
//! fakes.
//!
//! THE RULE FOR LOCKS: the cache is behind a lock that is held for a copy only and NEVER across a send. A send to Explorer can take
//! seconds, and while this thread waits, Windows delivers other programs' messages to it (they arrive in the same window procedure).
//! A lock held across the send would make that second entry wait for itself.

use parking_lot::Mutex;

use super::traits::{Raw, Relay, Relayed};
use crate::tray_wire::forward::{Method, Outcome, PickPolicy, SelfIdentity, WindowInfo, pick_real_tray, should_retry};

/// Looking up windows.
pub trait TrayFinder: Send + Sync {
    /// Every top-level window of class `Shell_TrayWnd`, topmost first, with its owner. The class is read back from the window, not
    /// assumed.
    fn tray_windows(&self) -> Vec<WindowInfo>;
    /// Whether `hwnd` is still a window of class `Shell_TrayWnd`.
    fn is_tray_window(&self, hwnd: isize) -> bool;
}

/// Delivering a message to a window of another process.
pub trait MsgSender: Send + Sync {
    /// Sends and waits (a time limit, and an early return when the target is hung). `Delivered` carries the target's answer.
    fn send(&self, target: isize, raw: Raw) -> Outcome;
    /// Posts. False when the window is gone.
    fn post(&self, target: isize, raw: Raw) -> bool;
}

struct State {
    me: SelfIdentity,
    target: Option<isize>,
}

pub struct RelayCore<F: TrayFinder, S: MsgSender> {
    finder: F,
    sender: S,
    policy: PickPolicy,
    state: Mutex<State>,
}

impl<F: TrayFinder, S: MsgSender> RelayCore<F, S> {
    pub fn new(finder: F, sender: S, me: SelfIdentity) -> RelayCore<F, S> {
        RelayCore { finder, sender, policy: PickPolicy::default(), state: Mutex::new(State { me, target: None }) }
    }

    /// Looks the real tray up now and remembers the answer.
    fn look_up(&self) -> Option<isize> {
        let me = self.state.lock().me.clone();
        let found = pick_real_tray(&self.finder.tray_windows(), &me, self.policy);
        self.state.lock().target = found;
        found
    }
}

impl<F: TrayFinder, S: MsgSender> Relay for RelayCore<F, S> {
    fn relay(&self, raw: Raw, method: Method) -> Relayed {
        let Some(target) = self.target() else { return Relayed::NoTarget };
        match method {
            Method::Post => Relayed::Done(if self.sender.post(target, raw) { Outcome::Posted } else { Outcome::TargetGone }),
            // (`Local` never reaches a relay: the window procedure keeps those)
            Method::Send | Method::Local => {
                let outcome = self.sender.send(target, raw);
                if should_retry(outcome) {
                    // Explorer's window vanished while we held its handle: look once more, send once more. The first send was
                    // not delivered (the window was gone), so nothing is added twice.
                    return match self.look_up() {
                        Some(again) => Relayed::Done(self.sender.send(again, raw)),
                        None => Relayed::NoTarget,
                    };
                }
                Relayed::Done(outcome)
            }
        }
    }

    fn target(&self) -> Option<isize> {
        let cached = self.state.lock().target;
        if let Some(t) = cached
            && self.finder.is_tray_window(t)
        {
            return Some(t);
        }
        self.look_up()
    }

    fn invalidate(&self) {
        self.state.lock().target = None;
    }

    fn set_self(&self, hwnd: isize) {
        self.state.lock().me.hwnd = hwnd;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray_wire::TRAY_CLASS;
    use parking_lot::Mutex as PMutex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn win(hwnd: isize, pid: u32, exe: Option<&str>) -> WindowInfo {
        WindowInfo { hwnd, class: TRAY_CLASS.into(), pid, exe: exe.map(str::to_string) }
    }

    #[derive(Clone, Default)]
    struct FakeFinder {
        windows: Arc<PMutex<Vec<WindowInfo>>>,
        lookups: Arc<AtomicUsize>,
    }

    impl TrayFinder for FakeFinder {
        fn tray_windows(&self) -> Vec<WindowInfo> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            self.windows.lock().clone()
        }
        fn is_tray_window(&self, hwnd: isize) -> bool {
            self.windows.lock().iter().any(|w| w.hwnd == hwnd)
        }
    }

    #[derive(Clone)]
    struct FakeSender {
        sent: Arc<PMutex<Vec<(isize, Raw)>>>,
        posted: Arc<PMutex<Vec<(isize, Raw)>>>,
        /// Targets that are gone: sending to them fails with `TargetGone`.
        gone: Arc<PMutex<Vec<isize>>>,
        answer: Arc<PMutex<Outcome>>,
    }

    impl FakeSender {
        fn new() -> FakeSender {
            FakeSender { sent: Default::default(), posted: Default::default(), gone: Default::default(), answer: Arc::new(PMutex::new(Outcome::Delivered(1))) }
        }
    }

    impl MsgSender for FakeSender {
        fn send(&self, target: isize, raw: Raw) -> Outcome {
            self.sent.lock().push((target, raw));
            if self.gone.lock().contains(&target) {
                Outcome::TargetGone
            } else {
                *self.answer.lock()
            }
        }
        fn post(&self, target: isize, raw: Raw) -> bool {
            self.posted.lock().push((target, raw));
            !self.gone.lock().contains(&target)
        }
    }

    fn me() -> SelfIdentity {
        SelfIdentity { hwnd: 100, pid: 5000, exe: "rustyfiles.exe".into() }
    }

    fn raw() -> Raw {
        Raw { msg: 0x4A, wparam: 7, lparam: 0x1234 }
    }

    fn setup(windows: Vec<WindowInfo>) -> (RelayCore<FakeFinder, FakeSender>, FakeFinder, FakeSender) {
        let f = FakeFinder::default();
        *f.windows.lock() = windows;
        let s = FakeSender::new();
        (RelayCore::new(f.clone(), s.clone(), me()), f, s)
    }

    #[test]
    fn forwards_to_explorers_window_unchanged_and_returns_its_answer() {
        let (r, _f, s) = setup(vec![win(100, 5000, Some("rustyfiles.exe")), win(200, 1234, Some("explorer.exe"))]);
        *s.answer.lock() = Outcome::Delivered(42);
        assert_eq!(r.relay(raw(), Method::Send), Relayed::Done(Outcome::Delivered(42)));
        assert_eq!(s.sent.lock().as_slice(), &[(200, raw())], "the same message, wParam and lParam as it came");
    }

    #[test]
    fn never_forwards_to_itself() {
        let (r, _f, s) = setup(vec![win(100, 5000, Some("rustyfiles.exe"))]);
        assert_eq!(r.relay(raw(), Method::Send), Relayed::NoTarget);
        assert!(s.sent.lock().is_empty());
        assert_eq!(r.target(), None);
    }

    #[test]
    fn the_target_is_cached_and_checked_before_each_send() {
        let (r, f, _s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        for _ in 0..5 {
            r.relay(raw(), Method::Send);
        }
        assert_eq!(f.lookups.load(Ordering::SeqCst), 1, "one enumeration, then the cache");
    }

    #[test]
    fn an_explorer_that_restarted_is_found_again() {
        let (r, f, s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        r.relay(raw(), Method::Send);
        // Explorer is restarted: the old window is gone, a new one exists
        *f.windows.lock() = vec![win(300, 2222, Some("explorer.exe"))];
        assert_eq!(r.relay(raw(), Method::Send), Relayed::Done(Outcome::Delivered(1)));
        assert_eq!(s.sent.lock().last().unwrap().0, 300);
        assert_eq!(r.target(), Some(300));
    }

    #[test]
    fn a_window_that_dies_during_the_send_is_looked_up_once_more_and_the_message_sent_once_more() {
        let (r, f, s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        assert_eq!(r.target(), Some(200));
        // the handle is still listed (cache check passes) but sending says it is gone; a new Explorer window is there
        s.gone.lock().push(200);
        *f.windows.lock() = vec![win(200, 1234, Some("explorer.exe")), win(300, 2222, Some("explorer.exe"))];
        // 200 is first and the best: the lookup returns 200 again, and the retry fails the same way
        assert_eq!(r.relay(raw(), Method::Send), Relayed::Done(Outcome::TargetGone));
        assert_eq!(s.sent.lock().len(), 2, "one retry, no more");
        // now the dead one is no longer listed
        *f.windows.lock() = vec![win(300, 2222, Some("explorer.exe"))];
        r.invalidate();
        assert_eq!(r.relay(raw(), Method::Send), Relayed::Done(Outcome::Delivered(1)));
    }

    #[test]
    fn when_explorer_is_gone_for_good_there_is_no_target() {
        let (r, f, s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        assert!(r.target().is_some());
        s.gone.lock().push(200);
        f.windows.lock().clear();
        assert_eq!(r.relay(raw(), Method::Send), Relayed::NoTarget);
    }

    #[test]
    fn a_timeout_is_reported_and_not_retried() {
        let (r, _f, s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        *s.answer.lock() = Outcome::TimedOut;
        assert_eq!(r.relay(raw(), Method::Send), Relayed::Done(Outcome::TimedOut));
        assert_eq!(s.sent.lock().len(), 1, "a hung target is not asked twice: the sender is waiting");
    }

    #[test]
    fn posting_uses_post() {
        let (r, _f, s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        assert_eq!(r.relay(raw(), Method::Post), Relayed::Done(Outcome::Posted));
        assert_eq!(s.posted.lock().len(), 1);
        assert!(s.sent.lock().is_empty());
        s.gone.lock().push(200);
        assert_eq!(r.relay(raw(), Method::Post), Relayed::Done(Outcome::TargetGone));
    }

    #[test]
    fn our_own_hwnd_is_set_after_the_window_exists() {
        let (r, f, _s) = setup(vec![win(555, 9, None)]);
        // a window of unknown owner is an acceptable target while running
        assert_eq!(r.target(), Some(555));
        r.invalidate();
        r.set_self(555);
        assert_eq!(r.target(), None, "now it is recognised as ours");
        assert!(f.lookups.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn invalidate_makes_the_next_message_look_again() {
        let (r, f, _s) = setup(vec![win(200, 1234, Some("explorer.exe"))]);
        r.relay(raw(), Method::Send);
        r.invalidate();
        r.relay(raw(), Method::Send);
        assert_eq!(f.lookups.load(Ordering::SeqCst), 2);
    }
}
