//! What the hook thread and the interface thread share: a handful of atomics. The hook callback reads and writes ONLY these (and its
//! own fields): no lock, no allocation, nothing that can wait. Everything the interface thread wants the hook to know goes in here
//! (the settings, the heartbeat, "a full-screen program is in front", "the switcher closed"); everything the hook wants the
//! interface to know goes out through the queue (`queue.rs`) or is read back from here (`believed`, `mask_failures`).
//!
//! FAIL OPEN. The interface thread stamps the heartbeat every [`HEARTBEAT_EVERY_MS`]. When the stamp is older than
//! [`STALE_AFTER_MS`] (four missed beats: the interface is stuck, busy or gone) - or has never been made - the layer passes EVERY
//! key and swallows nothing, so a stuck rustyfiles can never take Alt+Tab or the Windows key away. The same happens while a
//! full-screen program is in front (the dock's own full-screen test, stamped with the heartbeat): our panels do not open above one,
//! so a swallowed key would do nothing at all.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;
use std::time::Instant;

use super::queue::{Posted, Queue};

/// How often the interface thread stamps the heartbeat.
pub const HEARTBEAT_EVERY_MS: u64 = 250;
/// A heartbeat older than this means the interface is not there: the layer fails open.
pub const STALE_AFTER_MS: u64 = 1_000;

/// Milliseconds since the layer's clock started. Real, or set by hand in tests.
pub struct Clock {
    epoch: Instant,
    manual: Option<AtomicU64>,
}

impl Clock {
    pub fn real() -> Clock {
        Clock { epoch: Instant::now(), manual: None }
    }

    /// A clock that only moves when [`Clock::set`] says so (for tests).
    pub fn manual() -> Clock {
        Clock { epoch: Instant::now(), manual: Some(AtomicU64::new(0)) }
    }

    pub fn now_ms(&self) -> u64 {
        match &self.manual {
            Some(m) => m.load(Ordering::Relaxed),
            None => self.epoch.elapsed().as_millis() as u64,
        }
    }

    /// Sets a manual clock (does nothing to a real one).
    pub fn set(&self, ms: u64) {
        if let Some(m) = &self.manual {
            m.store(ms, Ordering::Relaxed);
        }
    }
}

/// The keys whose state the layer keeps a belief about, and which the heartbeat compares with the keyboard's own state: a
/// disagreement that lasts means the hook is no longer being called (see `service.rs`). Tab is NOT among them: a Tab the hook
/// swallows may or may not show in the keyboard's state (not run here), and a comparison that fails during Alt+Tab would reinstall the
/// hook in the middle of a chord. Alt and the Windows keys are never swallowed, so their state is the same either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracked {
    /// Either Windows key.
    pub win: bool,
    /// Either Alt key.
    pub alt: bool,
}

impl Tracked {
    pub const fn bits(self) -> u8 {
        (self.win as u8) | ((self.alt as u8) << 1)
    }

    pub const fn from_bits(b: u8) -> Tracked {
        Tracked { win: b & 1 != 0, alt: b & 2 != 0 }
    }
}

/// "The switcher is gone" (the interface to the hook): which opening it was, and whether the chord should give up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reset {
    pub seq: u16,
    /// The switcher could not be opened: stop swallowing Alt+Tab until Alt is released.
    pub give_up: bool,
}

pub struct Shared {
    clock: Arc<Clock>,
    win_on: AtomicBool,
    alt_on: AtomicBool,
    /// Win+E and Win+D are ours (set by the interface: only while no Explorer is the shell).
    combos_on: AtomicBool,
    log_only: AtomicBool,
    fullscreen: AtomicBool,
    /// The clock's reading at the last heartbeat, plus one (0: never stamped).
    heartbeat: AtomicU64,
    /// A [`Reset`] from the interface, packed: `seq << 1 | give_up`; 0: none.
    chord_reset: AtomicU32,
    /// Counts the openings of the switcher, across hook installations.
    open_seq: AtomicU32,
    /// What the hook last saw of the tracked keys that reached the system.
    believed: AtomicU8,
    /// The chord is open as a HELD one (the layer believes Alt is down and a switcher is up): written by the callback after each
    /// event, read at the beats, which compare it with the keyboard's own state of Alt.
    chord_held: AtomicBool,
    /// Masks the system refused (the key could not be sent).
    mask_failed: AtomicU32,
    queue: Queue,
    /// The thread that reads the queue; woken by every post.
    waker: OnceLock<Thread>,
}

impl Shared {
    pub fn new(clock: Arc<Clock>, log_only: bool) -> Shared {
        Shared {
            clock,
            win_on: AtomicBool::new(false),
            alt_on: AtomicBool::new(false),
            combos_on: AtomicBool::new(false),
            log_only: AtomicBool::new(log_only),
            fullscreen: AtomicBool::new(false),
            heartbeat: AtomicU64::new(0),
            chord_reset: AtomicU32::new(0),
            open_seq: AtomicU32::new(0),
            believed: AtomicU8::new(0),
            chord_held: AtomicBool::new(false),
            mask_failed: AtomicU32::new(0),
            queue: Queue::new(),
            waker: OnceLock::new(),
        }
    }

    pub fn clock(&self) -> &Arc<Clock> {
        &self.clock
    }

    // ---- the interface thread's side

    pub fn set_wanted(&self, win_key: bool, own_switcher: bool) {
        self.win_on.store(win_key, Ordering::Relaxed);
        self.alt_on.store(own_switcher, Ordering::Relaxed);
    }

    /// Win+E and Win+D are to be taken (they need `win_on` as well).
    pub fn set_combos(&self, on: bool) {
        self.combos_on.store(on, Ordering::Relaxed);
    }

    pub fn set_waker(&self, t: Thread) {
        let _ = self.waker.set(t);
    }

    /// The interface thread is alive: stamps the heartbeat and tells the layer whether a full-screen program is in front.
    pub fn beat(&self, fullscreen: bool) {
        self.fullscreen.store(fullscreen, Ordering::Relaxed);
        self.heartbeat.store(self.clock.now_ms().saturating_add(1), Ordering::Release);
    }

    /// The switcher closed by itself (a card was chosen, Escape, a click, the foreground was lost): the chord of that opening is over.
    pub fn chord_closed(&self, seq: u16) {
        self.chord_reset.store(u32::from(seq) << 1, Ordering::Release);
    }

    /// The switcher could not be opened: the chord of that opening is given up at once, and Alt+Tab goes to Windows until Alt is
    /// released.
    pub fn give_up(&self, seq: u16) {
        self.chord_reset.store((u32::from(seq) << 1) | 1, Ordering::Release);
    }

    /// Ends the chord of the latest opening from the interface side, without waiting for a key event or for the switcher to say so
    /// (the layer notices at its next event). Used when the keyboard shows Alt up while the chord believes it is held.
    pub fn close_current_chord(&self) {
        let n = self.open_seq.load(Ordering::Relaxed);
        self.chord_closed(((n & 0xFFFF) as u16).max(1));
    }

    /// Whether the chord is open as a held one, as the callback last left it.
    pub fn chord_held(&self) -> bool {
        self.chord_held.load(Ordering::Relaxed)
    }

    pub fn believed(&self) -> Tracked {
        Tracked::from_bits(self.believed.load(Ordering::Relaxed))
    }

    pub fn mask_failures(&self) -> u32 {
        self.mask_failed.load(Ordering::Relaxed)
    }

    pub fn log_only(&self) -> bool {
        self.log_only.load(Ordering::Relaxed)
    }

    /// The dispatcher's side of the queue.
    pub fn pop(&self) -> Option<Posted> {
        self.queue.pop()
    }

    pub fn take_lost(&self) -> bool {
        self.queue.take_lost()
    }

    // ---- the hook callback's side (atomics only)

    pub fn win_on(&self) -> bool {
        self.win_on.load(Ordering::Relaxed)
    }

    pub fn alt_on(&self) -> bool {
        self.alt_on.load(Ordering::Relaxed)
    }

    pub fn combos_on(&self) -> bool {
        self.combos_on.load(Ordering::Relaxed)
    }

    /// True when the layer must pass every key: the heartbeat is stale or was never made, or a full-screen program is in front.
    pub fn standing_down(&self) -> bool {
        if self.fullscreen.load(Ordering::Relaxed) {
            return true;
        }
        let beat = self.heartbeat.load(Ordering::Acquire);
        beat == 0 || self.clock.now_ms().saturating_add(1).saturating_sub(beat) > STALE_AFTER_MS
    }

    /// Posts to the interface and wakes the dispatcher. Never blocks, never allocates.
    pub fn post(&self, p: Posted) {
        self.queue.push(p);
        if let Some(t) = self.waker.get() {
            t.unpark();
        }
    }

    /// The next opening's number (never 0).
    pub fn next_seq(&self) -> u16 {
        let n = self.open_seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        ((n & 0xFFFF) as u16).max(1)
    }

    pub fn take_reset(&self) -> Option<Reset> {
        // one atomic step: a reset the interface thread writes between a load and a store of zero would be wiped
        let v = self.chord_reset.swap(0, Ordering::AcqRel);
        if v == 0 {
            return None;
        }
        Some(Reset { seq: (v >> 1) as u16, give_up: v & 1 != 0 })
    }

    pub fn set_chord_held(&self, held: bool) {
        self.chord_held.store(held, Ordering::Relaxed);
    }

    pub fn set_believed(&self, t: Tracked) {
        self.believed.store(t.bits(), Ordering::Relaxed);
    }

    pub fn note_mask_failed(&self) {
        self.mask_failed.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> (Arc<Clock>, Shared) {
        let clock = Arc::new(Clock::manual());
        (clock.clone(), Shared::new(clock, false))
    }

    #[test]
    fn a_layer_whose_interface_never_stamped_the_heartbeat_stands_down() {
        let (_, s) = shared();
        assert!(s.standing_down(), "no heartbeat yet: every key passes");
    }

    #[test]
    fn the_heartbeat_keeps_it_up_and_going_stale_stands_it_down_and_a_new_beat_brings_it_back() {
        let (clock, s) = shared();
        clock.set(5_000);
        s.beat(false);
        assert!(!s.standing_down());
        clock.set(5_000 + STALE_AFTER_MS);
        assert!(!s.standing_down(), "exactly at the limit is still alive");
        clock.set(5_000 + STALE_AFTER_MS + 1);
        assert!(s.standing_down(), "one millisecond past the limit");
        clock.set(9_000);
        s.beat(false);
        assert!(!s.standing_down());
    }

    #[test]
    fn a_beat_at_time_zero_counts_as_a_beat() {
        let (_, s) = shared();
        s.beat(false); // the clock reads 0: stored as 1, not as "never"
        assert!(!s.standing_down());
    }

    #[test]
    fn a_full_screen_program_in_front_stands_it_down_and_leaving_it_brings_it_back() {
        let (clock, s) = shared();
        clock.set(100);
        s.beat(true);
        assert!(s.standing_down());
        s.beat(false);
        assert!(!s.standing_down());
    }

    #[test]
    fn a_reset_is_read_once_and_carries_its_opening_and_whether_to_give_up() {
        let (_, s) = shared();
        assert_eq!(s.take_reset(), None);
        s.chord_closed(7);
        assert_eq!(s.take_reset(), Some(Reset { seq: 7, give_up: false }));
        assert_eq!(s.take_reset(), None);
        s.give_up(65_535);
        assert_eq!(s.take_reset(), Some(Reset { seq: 65_535, give_up: true }));
    }

    #[test]
    fn a_reset_written_while_another_is_being_taken_is_never_lost() {
        // The interface thread writes a reset at any moment, also at the instant the hook thread takes the previous one. Taking must be
        // one atomic step: with a load and then a store of zero, a write landing between the two was wiped, and the chord stayed open
        // for a switcher that was gone (Tab, the arrows, Enter and Escape swallowed everywhere). Probabilistic against the old code
        // (the window is a few instructions wide); deterministic against the new one.
        use std::sync::atomic::AtomicBool;
        let (_, s) = shared();
        let s = Arc::new(s);
        let done = Arc::new(AtomicBool::new(false));
        let last_seen = Arc::new(AtomicU32::new(0));
        let reader = {
            let (s, done, last_seen) = (s.clone(), done.clone(), last_seen.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if let Some(r) = s.take_reset() {
                        last_seen.store(u32::from(r.seq), Ordering::Release);
                    }
                }
                if let Some(r) = s.take_reset() {
                    last_seen.store(u32::from(r.seq), Ordering::Release);
                }
            })
        };
        let mut lost = 0;
        for i in 1..=200_000u32 {
            let seq = (i % 60_000 + 1) as u16;
            s.chord_closed(seq.wrapping_sub(1).max(1));
            s.chord_closed(seq);
            // the newest write must reach the reader, whatever the reader was in the middle of
            let mut spins = 0u32;
            while last_seen.load(Ordering::Acquire) != u32::from(seq) {
                spins += 1;
                if spins > 5_000_000 {
                    lost += 1;
                    break;
                }
                std::hint::spin_loop();
            }
            if lost > 0 {
                break;
            }
        }
        done.store(true, Ordering::Release);
        reader.join().unwrap();
        assert_eq!(lost, 0, "a reset was overwritten by the take that was reading the one before");
    }

    #[test]
    fn opening_numbers_start_at_one_and_never_come_out_zero() {
        let (_, s) = shared();
        assert_eq!(s.next_seq(), 1);
        assert_eq!(s.next_seq(), 2);
        let s2 = Shared::new(Arc::new(Clock::manual()), false);
        s2.open_seq.store(65_535, Ordering::Relaxed);
        assert_eq!(s2.next_seq(), 1, "wrapped past 0");
    }

    #[test]
    fn tracked_keys_pack_into_bits_and_back() {
        for b in 0..4u8 {
            assert_eq!(Tracked::from_bits(b).bits(), b);
        }
        assert_eq!(Tracked { win: true, alt: false }.bits(), 1);
    }

    #[test]
    fn the_wanted_flags_and_the_log_only_flag_are_read_back() {
        let (_, s) = shared();
        assert!(!s.win_on() && !s.alt_on() && !s.log_only());
        s.set_wanted(true, false);
        assert!(s.win_on() && !s.alt_on());
        assert!(Shared::new(Arc::new(Clock::manual()), true).log_only());
    }
}
