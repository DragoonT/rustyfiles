//! What the window procedure's thread and the interface share: the model behind one lock, a generation number readable without it, the
//! counters, and the bounded queue of event hints.
//!
//! The handler holds the lock for ONE model update (a copy of a few fields) and never while it waits for Explorer or reads an icon.
//! The interface takes a [`Snapshot`] (a clone: icon pictures are shared, not copied) and reads the generation with no lock at all.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use parking_lot::Mutex;

use super::model::{ApplyOutcome, IconId, Prepared, Snapshot, TrayEvent, TrayModel};
use crate::tray_wire::notify::{Nim, NotifyIconRequest};
use crate::tray_wire::{IconKey, Rect};

/// How many event hints may wait for the interface. A full queue drops the newest: the model and its generation are the truth.
pub const EVENT_QUEUE: usize = 1024;

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        /// Counts of what the host has seen and done. Only numbers: never a tip, a name or a picture.
        #[derive(Default)]
        pub struct Counters { $(pub $name: AtomicU64),* }

        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct Counts { $(pub $name: u64),* }

        impl Counters {
            pub fn snapshot(&self) -> Counts { Counts { $($name: self.$name.load(Ordering::Relaxed)),* } }
        }

        impl Counts {
            /// One line for the log.
            pub fn summary(&self) -> String {
                let mut parts: Vec<String> = Vec::new();
                $( if self.$name != 0 { parts.push(format!("{} {}", stringify!($name), self.$name)); } )*
                if parts.is_empty() { "nothing yet".to_string() } else { parts.join(", ") }
            }
        }
    };
}

counters!(
    messages,
    notify,
    appbar,
    service,
    rect_queries,
    rect_answered,
    unknown_kind,
    unparsed,
    adds,
    re_adds,
    modifies,
    modify_unknown,
    adopted,
    adopted_version_guessed,
    deletes,
    delete_unknown,
    set_versions,
    version_refused,
    set_focus,
    icons_full,
    pixel_failures,
    balloons,
    dead_dropped,
    forwarded,
    forward_timed_out,
    forward_target_gone,
    no_target,
    other_messages_passed_on,
    handler_panics,
    clicks_sent,
    clicks_dropped,
    explorer_refused,
    local_registered,
    local_sent_by_others,
    raised,
    appbar_own,
    appbar_new_refused,
);

pub(crate) fn bump(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

pub(crate) struct State {
    pub model: TrayModel,
    /// The rectangle (screen coordinates) the interface reports for each icon, for `Shell_NotifyIconGetRect`.
    pub rects: HashMap<IconId, Rect>,
    /// LOG-ONLY mode: the keys of the icons seen, and nothing else about them.
    pub seen_keys: HashSet<IconKey>,
}

pub struct Shared {
    pub(crate) state: Mutex<State>,
    generation: AtomicU64,
    /// Beaten by the host thread's tick; a watcher on another thread reads it (`stall`).
    beat: AtomicU64,
    pub counters: Counters,
    events: Sender<TrayEvent>,
}

impl Shared {
    pub fn new() -> (Shared, Receiver<TrayEvent>) {
        let (tx, rx) = bounded(EVENT_QUEUE);
        let s = Shared {
            state: Mutex::new(State { model: TrayModel::new(), rects: HashMap::new(), seen_keys: HashSet::new() }),
            generation: AtomicU64::new(0),
            beat: AtomicU64::new(0),
            counters: Counters::default(),
            events: tx,
        };
        (s, rx)
    }

    /// The model's generation as of the last update, read without the lock.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// One beat of the host thread's tick.
    pub(crate) fn beat(&self) {
        self.beat.fetch_add(1, Ordering::Relaxed);
    }

    /// How many times the host thread has ticked.
    pub fn heartbeat(&self) -> u64 {
        self.beat.load(Ordering::Relaxed)
    }

    pub(crate) fn publish(&self, g: u64, events: &[TrayEvent]) {
        self.generation.store(g, Ordering::Release);
        for e in events {
            match self.events.try_send(*e) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                // (full: the interface is behind; it will see the change by the generation)
                Err(TrySendError::Full(_)) => {}
            }
        }
    }

    /// Applies one request under the lock (a copy of a few fields), then publishes the events outside it.
    pub(crate) fn apply(&self, req: &NotifyIconRequest, prep: Prepared) -> ApplyOutcome {
        let (out, g) = {
            let mut st = self.state.lock();
            let out = st.model.apply(req, prep);
            if let Some(id) = out.removed {
                st.rects.remove(&id);
            }
            (out, st.model.generation())
        };
        self.publish(g, &out.events);
        out
    }

    /// LOG-ONLY: notes the key of an icon (and forgets it at delete) without keeping anything else. Returns the answer a tray gives.
    pub(crate) fn note_key(&self, req: &NotifyIconRequest) -> bool {
        const LIMIT: usize = super::model::MAX_ICONS * 4;
        let key = req.data.key();
        let mut st = self.state.lock();
        match req.message {
            Nim::Add => {
                if st.seen_keys.contains(&key) || st.seen_keys.len() >= LIMIT {
                    false
                } else {
                    st.seen_keys.insert(key);
                    true
                }
            }
            Nim::Modify | Nim::SetVersion => st.seen_keys.contains(&key),
            Nim::Delete => st.seen_keys.remove(&key),
            Nim::SetFocus => true,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().model.snapshot()
    }

    /// Whether the model holds the icon this request names (one short look under the lock).
    pub(crate) fn knows(&self, req: &NotifyIconRequest) -> bool {
        self.state.lock().model.find(&req.data.key()).is_some()
    }

    /// The snapshot if the model changed since `last` (a generation the caller remembers), else `None` without taking the lock.
    pub fn snapshot_if_changed(&self, last: u64) -> Option<Snapshot> {
        if self.generation() == last {
            return None;
        }
        Some(self.snapshot())
    }

    /// How many icons the model holds, or in LOG-ONLY mode how many keys have been seen.
    pub fn icon_count(&self, log_only: bool) -> usize {
        let st = self.state.lock();
        if log_only { st.seen_keys.len() } else { st.model.len() }
    }

    /// Replaces the rectangles the interface reports (screen coordinates, one per icon on show). Icons no longer in the model are
    /// ignored.
    pub fn set_icon_rects(&self, rects: &[(IconId, Rect)]) {
        let mut st = self.state.lock();
        let ids: HashSet<IconId> = st.model.icons().iter().map(|i| i.id).collect();
        st.rects = rects.iter().filter(|(id, _)| ids.contains(id)).cloned().collect();
    }

    /// Removes an icon whose owner is gone. True if it was there.
    pub(crate) fn drop_icon(&self, id: IconId) -> bool {
        let (removed, g) = {
            let mut st = self.state.lock();
            st.rects.remove(&id);
            let r = st.model.remove(id);
            (r, st.model.generation())
        };
        if removed {
            self.publish(g, &[TrayEvent::Removed(id)]);
            bump(&self.counters.dead_dropped);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::test_util::*;
    use super::*;

    #[test]
    fn apply_publishes_the_generation_and_the_events() {
        let (s, rx) = Shared::new();
        assert_eq!(s.generation(), 0);
        s.apply(&add(1, 1), Prepared::default());
        assert_eq!(s.generation(), 1);
        assert!(matches!(rx.try_recv(), Ok(TrayEvent::Added(_))));
        let snap = s.snapshot_if_changed(0).unwrap();
        assert_eq!(snap.icons.len(), 1);
        assert!(s.snapshot_if_changed(snap.generation).is_none(), "nothing changed since");
    }

    #[test]
    fn a_full_queue_drops_events_but_the_generation_still_moves() {
        let (s, rx) = Shared::new();
        s.apply(&add(1000, 1), Prepared::default());
        // flood the queue with changes to one icon
        let mut d = data(1000, 1, crate::tray_wire::notify::nif::TIP);
        for n in 0..(EVENT_QUEUE * 2) {
            d.tip = format!("tip {n}");
            s.apply(&req(Nim::Modify, d.clone()), Prepared::default());
        }
        assert!(rx.len() <= EVENT_QUEUE);
        assert_eq!(rx.len(), EVENT_QUEUE, "the queue is full, not larger");
        let g = s.generation();
        assert!(g > EVENT_QUEUE as u64, "every change was counted");
    }

    #[test]
    fn rects_are_dropped_with_their_icon_and_unknown_ids_ignored() {
        let (s, _rx) = Shared::new();
        s.apply(&add(1, 1), Prepared::default());
        let id = s.snapshot().icons[0].id;
        s.set_icon_rects(&[(id, Rect { left: 1, top: 2, right: 3, bottom: 4 }), (IconId(999), Rect::default())]);
        assert_eq!(s.state.lock().rects.len(), 1);
        s.apply(&req(Nim::Delete, data(1, 1, 0)), Prepared::default());
        assert!(s.state.lock().rects.is_empty());
    }

    #[test]
    fn log_only_keeps_keys_and_nothing_else() {
        let (s, _rx) = Shared::new();
        assert!(s.note_key(&add(1, 1)));
        assert!(!s.note_key(&add(1, 1)));
        assert!(s.note_key(&add(2, 1)));
        assert_eq!(s.icon_count(true), 2);
        assert_eq!(s.icon_count(false), 0, "the model itself stays empty");
        assert!(s.note_key(&req(Nim::Modify, data(1, 1, 0))));
        assert!(!s.note_key(&req(Nim::Modify, data(9, 9, 0))));
        assert!(s.note_key(&req(Nim::Delete, data(1, 1, 0))));
        assert!(!s.note_key(&req(Nim::Delete, data(1, 1, 0))));
        assert_eq!(s.icon_count(true), 1);
    }

    #[test]
    fn counts_print_only_what_happened() {
        let c = Counters::default();
        assert_eq!(c.snapshot().summary(), "nothing yet");
        bump(&c.adds);
        bump(&c.adds);
        bump(&c.forwarded);
        assert_eq!(c.snapshot().summary(), "adds 2, forwarded 1");
    }
}
