//! When the engine had to fall back to Desktop Duplication because a Windows.Graphics.Capture session could not be made, how often it
//! tries the session again.
//!
//! WHAT WAS SEEN (the user's trial without Explorer): the session could not make a capture item for the monitor ("Could not capture the
//! given monitor"), the engine fell back to Desktop Duplication, and that fallback hides every rustyfiles window from screenshots,
//! recordings and screen sharing (`WDA_EXCLUDEFROMCAPTURE`); the user was sharing the screen and the dock was missing from the stream.
//! The engine chose the backend ONCE per start and kept it for as long as glass was on screen, so a failure that was only passing (the
//! shell not there yet, a display still waking up) cost the stream for the rest of the run.
//!
//! NOW. A fallback that came from a SESSION failure (not from the system saying no to the border-free capture, which is a decision and
//! not a failure) is looked at again: after [`PAUSES`] and then every [`LAST_PAUSE`], the engine asks the permission and tries a
//! session on a monitor that has glass in view; when one opens, the engine goes back to sessions for all its windows and the windows
//! are visible to captures again. Each try costs one permission read (about 3 ms, a WinRT call) and, when the permission is there, one
//! session made and, when it works, kept. While the engine does not use Desktop Duplication nothing of this runs.

use std::time::{Duration, Instant};

/// The pauses before the first tries.
pub(super) const PAUSES: [Duration; 3] = [Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)];
/// The pause between tries after those.
pub(super) const LAST_PAUSE: Duration = Duration::from_secs(300);

/// The schedule of one fallback.
#[derive(Debug)]
pub(super) struct SessionRetry {
    failures: usize,
    next: Instant,
}

impl SessionRetry {
    /// The fallback began at `now`.
    pub(super) fn new(now: Instant) -> SessionRetry {
        SessionRetry { failures: 0, next: now + PAUSES[0] }
    }

    /// Whether a try may be made now.
    pub(super) fn due(&self, now: Instant) -> bool {
        now >= self.next
    }

    /// A try was made and failed (or the permission was not there): the next one is after the next pause.
    pub(super) fn failed(&mut self, now: Instant) {
        self.failures += 1;
        self.next = now + PAUSES.get(self.failures).copied().unwrap_or(LAST_PAUSE);
    }

    pub(super) fn failures(&self) -> usize {
        self.failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_try_is_not_made_at_once_and_the_pauses_grow() {
        let t0 = Instant::now();
        let mut r = SessionRetry::new(t0);
        assert!(!r.due(t0), "the failure that caused the fallback is not retried in the same breath");
        assert!(!r.due(t0 + Duration::from_secs(9)));
        assert!(r.due(t0 + Duration::from_secs(10)));
        let mut now = t0 + Duration::from_secs(10);
        r.failed(now);
        assert!(!r.due(now + Duration::from_secs(29)));
        assert!(r.due(now + Duration::from_secs(30)));
        now += Duration::from_secs(30);
        r.failed(now);
        assert!(!r.due(now + Duration::from_secs(59)));
        assert!(r.due(now + Duration::from_secs(60)));
        now += Duration::from_secs(60);
        r.failed(now);
        // from here on: every five minutes, for ever
        for _ in 0..5 {
            assert!(!r.due(now + LAST_PAUSE - Duration::from_secs(1)));
            assert!(r.due(now + LAST_PAUSE));
            now += LAST_PAUSE;
            r.failed(now);
        }
        assert_eq!(r.failures(), 8);
    }

    #[test]
    fn a_late_look_does_not_bring_the_next_one_forward() {
        // the engine looks when a turn comes round, which may be later than due: the pause counts from the failure, not from the schedule
        let t0 = Instant::now();
        let mut r = SessionRetry::new(t0);
        let late = t0 + Duration::from_secs(25);
        assert!(r.due(late));
        r.failed(late);
        assert!(!r.due(late + Duration::from_secs(29)));
        assert!(r.due(late + Duration::from_secs(30)));
    }
}
