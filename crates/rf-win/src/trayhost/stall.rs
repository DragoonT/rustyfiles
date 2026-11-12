//! Noticing a host thread that has stopped ticking. The host's own thread beats a counter every 250 ms (`Shared::beat`); a watcher on
//! another thread (the app's reporting thread) feeds the counter to a [`StallWatch`] now and then, and the watch says when the beat
//! has been still for too long and when it moves again.
//!
//! WHAT THIS IS FOR, AND WHAT IT IS NOT. A window can only be destroyed by the thread that owns it, so when that thread is stuck no
//! other thread can take our `Shell_TrayWnd` away: the watch cannot fix a stuck host, it only makes sure that somebody notices
//! (a line in the log with a number of seconds) instead of the host failing silently. Every wait on the host thread is bounded
//! (the forward to Explorer 4 s, `AllowSetForegroundWindow` and `SendNotifyMessage` do not wait), so a real stall is not expected.
//! The remedy for a stall that does happen is to end the dock's process (the window goes with it, Explorer's window is first again).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallEvent {
    /// The beat has not moved for this many seconds (reported once per stall).
    Stalled { secs: u64 },
    /// The beat moves again after a reported stall.
    Recovered,
}

pub struct StallWatch {
    limit_secs: u64,
    last_beat: Option<u64>,
    last_change_at: u64,
    reported: bool,
}

impl StallWatch {
    pub fn new(limit_secs: u64) -> StallWatch {
        StallWatch { limit_secs, last_beat: None, last_change_at: 0, reported: false }
    }

    /// `beat` is the counter now, `now_secs` any clock that only goes forward.
    pub fn observe(&mut self, beat: u64, now_secs: u64) -> Option<StallEvent> {
        if self.last_beat != Some(beat) {
            self.last_beat = Some(beat);
            self.last_change_at = now_secs;
            return if std::mem::take(&mut self.reported) { Some(StallEvent::Recovered) } else { None };
        }
        let still = now_secs.saturating_sub(self.last_change_at);
        if still >= self.limit_secs && !self.reported {
            self.reported = true;
            return Some(StallEvent::Stalled { secs: still });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_beat_that_keeps_moving_is_never_a_stall() {
        let mut w = StallWatch::new(15);
        for t in 0..200u64 {
            assert_eq!(w.observe(t, t * 5), None);
        }
    }

    #[test]
    fn a_beat_that_stops_is_reported_once_after_the_limit() {
        let mut w = StallWatch::new(15);
        assert_eq!(w.observe(7, 0), None);
        assert_eq!(w.observe(7, 5), None);
        assert_eq!(w.observe(7, 10), None);
        assert_eq!(w.observe(7, 15), Some(StallEvent::Stalled { secs: 15 }));
        assert_eq!(w.observe(7, 20), None, "once per stall");
        assert_eq!(w.observe(7, 600), None);
    }

    #[test]
    fn a_beat_that_moves_again_is_reported_as_recovered_once() {
        let mut w = StallWatch::new(10);
        w.observe(1, 0);
        assert!(matches!(w.observe(1, 10), Some(StallEvent::Stalled { .. })));
        assert_eq!(w.observe(2, 15), Some(StallEvent::Recovered));
        assert_eq!(w.observe(3, 20), None);
        // and a second stall is reported again
        assert_eq!(w.observe(3, 30), Some(StallEvent::Stalled { secs: 10 }));
    }

    #[test]
    fn a_recovery_with_nothing_reported_says_nothing() {
        let mut w = StallWatch::new(10);
        w.observe(1, 0);
        assert_eq!(w.observe(2, 5), None);
    }
}
