//! Monitors come and go (a cable, a dock, sleep, a resolution change): which cover windows to make, remove or place again, and the
//! debounce that waits for the burst of display messages to end before anything is done. Pure.

use crate::desktop::geom::Rect;

/// A monitor the desktop should cover. `key` is the stable device key the layout store uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub key: String,
    /// The monitor's whole rectangle, virtual-screen device pixels.
    pub rect: Rect,
    pub scale_percent: u32,
    /// The monitor's work area (it decides the icon grid): a change of it is not a reason to move the window, but the view must know.
    pub work: Rect,
}

/// A window we have, and the monitor it was last made or placed for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Have {
    pub key: String,
    pub rect: Rect,
    pub scale_percent: u32,
    pub work: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// A monitor without a window.
    Create(String),
    /// A window whose monitor is gone.
    Remove(String),
    /// The monitor is still there but its rectangle or scale changed: the window is placed again (and its view told).
    Place(String),
    /// Only the work area changed: the window stays, its view lays the icons out again.
    Relayout(String),
}

/// What to do so that there is exactly one window per monitor, each at its monitor. Removals first (a window of a monitor that left
/// must not be in the way of one that arrives in its place), then creations in the order of `want`.
pub fn reconcile(have: &[Have], want: &[Want]) -> Vec<Op> {
    let mut ops = Vec::new();
    for h in have {
        if !want.iter().any(|w| w.key == h.key) {
            ops.push(Op::Remove(h.key.clone()));
        }
    }
    for w in want {
        match have.iter().find(|h| h.key == w.key) {
            None => ops.push(Op::Create(w.key.clone())),
            Some(h) if h.rect != w.rect || h.scale_percent != w.scale_percent => ops.push(Op::Place(w.key.clone())),
            Some(h) if h.work != w.work => ops.push(Op::Relayout(w.key.clone())),
            Some(_) => {}
        }
    }
    ops
}

/// Waits for a burst of events to end: ready `quiet_ms` after the last one, or `max_ms` after the first, whichever comes first. The
/// caller supplies the clock (milliseconds from any fixed point), so that it can be tested. One display change arrives as a dozen
/// messages (`WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE`, a work-area change for each monitor), and while a monitor wakes up its
/// rectangle can be wrong for a moment: nothing should be made or moved until it has been quiet.
#[derive(Clone, Copy, Debug, Default)]
pub struct Debounce {
    quiet_ms: u64,
    max_ms: u64,
    first: Option<u64>,
    last: Option<u64>,
}

impl Debounce {
    pub const fn new(quiet_ms: u64, max_ms: u64) -> Debounce {
        Debounce { quiet_ms, max_ms, first: None, last: None }
    }

    /// An event arrived.
    pub fn poke(&mut self, now_ms: u64) {
        self.first.get_or_insert(now_ms);
        self.last = Some(now_ms);
    }

    /// Something is waiting.
    pub fn pending(&self) -> bool {
        self.first.is_some()
    }

    /// Whether the work should be done now.
    pub fn due(&self, now_ms: u64) -> bool {
        match (self.first, self.last) {
            (Some(first), Some(last)) => now_ms.saturating_sub(last) >= self.quiet_ms || now_ms.saturating_sub(first) >= self.max_ms,
            _ => false,
        }
    }

    /// How long to wait before looking again (`None`: nothing is waiting).
    pub fn wait_ms(&self, now_ms: u64) -> Option<u64> {
        let (first, last) = (self.first?, self.last?);
        let by_quiet = (last + self.quiet_ms).saturating_sub(now_ms);
        let by_max = (first + self.max_ms).saturating_sub(now_ms);
        Some(by_quiet.min(by_max))
    }

    /// The work was done.
    pub fn done(&mut self) {
        self.first = None;
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(key: &str, l: i32, t: i32, w: i32, h: i32, pct: u32) -> Want {
        Want { key: key.into(), rect: Rect::new(l, t, l + w, t + h), scale_percent: pct, work: Rect::new(0, 0, w, h - 48) }
    }
    fn have(w: &Want) -> Have {
        Have { key: w.key.clone(), rect: w.rect, scale_percent: w.scale_percent, work: w.work }
    }

    #[test]
    fn nothing_changed_nothing_to_do() {
        let a = want("A", 0, 0, 2560, 1440, 100);
        let b = want("B", 2560, -618, 1440, 2560, 100);
        assert_eq!(reconcile(&[have(&a), have(&b)], &[a, b]), []);
    }

    #[test]
    fn a_monitor_plugged_in_gets_a_window_and_one_unplugged_loses_it() {
        let a = want("A", 0, 0, 2560, 1440, 100);
        let b = want("B", 2560, -618, 1440, 2560, 100);
        assert_eq!(reconcile(&[have(&a)], &[a.clone(), b.clone()]), [Op::Create("B".into())]);
        assert_eq!(reconcile(&[have(&a), have(&b)], &[a]), [Op::Remove("B".into())]);
        assert_eq!(reconcile(&[], &[b]), [Op::Create("B".into())]);
    }

    #[test]
    fn a_monitor_that_changed_resolution_or_scale_or_place_is_placed_again_and_the_window_is_kept() {
        let a = want("A", 0, 0, 2560, 1440, 100);
        assert_eq!(reconcile(&[have(&a)], &[want("A", 0, 0, 1920, 1080, 100)]), [Op::Place("A".into())]);
        assert_eq!(reconcile(&[have(&a)], &[want("A", 0, 0, 2560, 1440, 125)]), [Op::Place("A".into())]);
        assert_eq!(reconcile(&[have(&a)], &[want("A", 1920, 0, 2560, 1440, 100)]), [Op::Place("A".into())]);
    }

    #[test]
    fn a_new_work_area_alone_does_not_move_the_window() {
        let a = want("A", 0, 0, 2560, 1440, 100);
        let mut moved = a.clone();
        moved.work = Rect::new(0, 0, 2560, 1380);
        assert_eq!(reconcile(&[have(&a)], &[moved]), [Op::Relayout("A".into())]);
    }

    #[test]
    fn a_monitor_swapped_for_another_removes_first_then_creates() {
        let a = want("A", 0, 0, 2560, 1440, 100);
        let c = want("C", 0, 0, 1920, 1080, 100);
        assert_eq!(reconcile(&[have(&a)], &[c]), [Op::Remove("A".into()), Op::Create("C".into())]);
    }

    #[test]
    fn all_monitors_gone_removes_every_window() {
        let a = want("A", 0, 0, 100, 100, 100);
        let b = want("B", 100, 0, 100, 100, 100);
        assert_eq!(reconcile(&[have(&a), have(&b)], &[]), [Op::Remove("A".into()), Op::Remove("B".into())]);
    }

    #[test]
    fn the_debounce_waits_for_quiet() {
        let mut d = Debounce::new(400, 2000);
        assert!(!d.pending() && !d.due(10_000) && d.wait_ms(0).is_none());
        d.poke(1000);
        assert!(d.pending() && !d.due(1100));
        d.poke(1300);
        d.poke(1500);
        assert!(!d.due(1800), "300 ms after the last poke");
        assert_eq!(d.wait_ms(1800), Some(100));
        assert!(d.due(1900));
        d.done();
        assert!(!d.pending() && !d.due(5000));
    }

    #[test]
    fn a_steady_stream_cannot_hold_the_work_off_for_ever() {
        let mut d = Debounce::new(400, 2000);
        let mut t = 0;
        while t < 1900 {
            d.poke(t);
            t += 100;
        }
        assert!(!d.due(1900));
        d.poke(1950);
        assert!(!d.due(1990));
        assert!(d.due(2000), "the cap, although the last poke was 50 ms ago");
        assert_eq!(d.wait_ms(1990), Some(10));
    }

    #[test]
    fn a_clock_that_went_back_is_not_a_wait_for_ever() {
        let mut d = Debounce::new(400, 2000);
        d.poke(5000);
        assert!(!d.due(100), "no panic and no early run");
        assert!(d.wait_ms(100).is_some());
    }
}
