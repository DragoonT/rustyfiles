//! What the hook callback tells the interface thread, and the queue it goes through. The callback must not block, allocate or take a
//! lock, so the queue is a ring of atomics with one writer (the hook thread) and one reader (the dispatcher thread): a post is a few
//! atomic stores, and when the ring is full the post is dropped and a flag says so (the reader then tells the interface that the
//! layer lost track, which closes the switcher).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use super::machine::Combo;
use crate::switcher::chord::{Dir, Effect, Scope};

/// What the callback posts. Plain `Copy` data that fits in one `u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posted {
    /// A clean tap of the Windows key ended: toggle the Start panel.
    StartTap,
    /// An effect of the Alt+Tab chord. `seq` numbers the openings: an `Open` carries the number of the opening it starts, the other
    /// effects the number of the opening they belong to.
    Chord { effect: Effect, seq: u16 },
    /// The layer lost track of the chord (it stood down or was switched off in the middle of one, the hook was installed again, or
    /// the queue overflowed): the switcher, if one is open, is to be closed.
    Lost,
    /// A chord of ours (Win+E, Win+D) was pressed and its key was taken.
    WinCombo(Combo),
}

const CODE_START: u32 = 1;
const CODE_LOST: u32 = 2;
const CODE_COMBO_EXPLORER: u32 = 3;
const CODE_COMBO_DESKTOP: u32 = 4;
const CODE_CHORD: u32 = 0x10;

fn effect_code(e: Effect) -> u32 {
    match e {
        Effect::Open { scope, backwards } => u32::from(scope == Scope::ThisApp) | (u32::from(backwards) << 1),
        Effect::Step { backwards } => 4 + u32::from(backwards),
        Effect::Move(Dir::Left) => 6,
        Effect::Move(Dir::Right) => 7,
        Effect::Move(Dir::Up) => 8,
        Effect::Move(Dir::Down) => 9,
        Effect::Choose => 10,
        Effect::Cancel => 11,
    }
}

fn effect_of(code: u32) -> Option<Effect> {
    Some(match code {
        0..=3 => Effect::Open { scope: if code & 1 == 0 { Scope::AllWindows } else { Scope::ThisApp }, backwards: code & 2 != 0 },
        4 => Effect::Step { backwards: false },
        5 => Effect::Step { backwards: true },
        6 => Effect::Move(Dir::Left),
        7 => Effect::Move(Dir::Right),
        8 => Effect::Move(Dir::Up),
        9 => Effect::Move(Dir::Down),
        10 => Effect::Choose,
        11 => Effect::Cancel,
        _ => return None,
    })
}

impl Posted {
    pub fn encode(self) -> u32 {
        match self {
            Posted::StartTap => CODE_START,
            Posted::Lost => CODE_LOST,
            Posted::WinCombo(Combo::Explorer) => CODE_COMBO_EXPLORER,
            Posted::WinCombo(Combo::Desktop) => CODE_COMBO_DESKTOP,
            Posted::Chord { effect, seq } => (u32::from(seq) << 16) | (CODE_CHORD + effect_code(effect)),
        }
    }

    pub fn decode(v: u32) -> Option<Posted> {
        match v & 0xFFFF {
            CODE_START => Some(Posted::StartTap),
            CODE_LOST => Some(Posted::Lost),
            CODE_COMBO_EXPLORER => Some(Posted::WinCombo(Combo::Explorer)),
            CODE_COMBO_DESKTOP => Some(Posted::WinCombo(Combo::Desktop)),
            c if c >= CODE_CHORD => effect_of(c - CODE_CHORD).map(|effect| Posted::Chord { effect, seq: (v >> 16) as u16 }),
            _ => None,
        }
    }
}

/// The ring's size. A chord and a few taps never come near it; it exists so that a reader that is stuck cannot make the writer wait.
pub const CAPACITY: usize = 64;

pub struct Queue {
    slots: [AtomicU32; CAPACITY],
    /// Written by the hook thread only.
    head: AtomicUsize,
    /// Written by the dispatcher thread only.
    tail: AtomicUsize,
    /// A post was dropped because the ring was full.
    lost: AtomicBool,
}

impl Queue {
    pub fn new() -> Queue {
        Queue { slots: std::array::from_fn(|_| AtomicU32::new(0)), head: AtomicUsize::new(0), tail: AtomicUsize::new(0), lost: AtomicBool::new(false) }
    }

    /// The writer's side. Never blocks and never allocates: a full ring drops the post and raises the `lost` flag.
    pub fn push(&self, p: Posted) {
        let head = self.head.load(Ordering::Relaxed);
        if head.wrapping_sub(self.tail.load(Ordering::Acquire)) >= CAPACITY {
            self.lost.store(true, Ordering::Release);
            return;
        }
        self.slots[head % CAPACITY].store(p.encode(), Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }

    /// The reader's side: the oldest post, if any.
    pub fn pop(&self) -> Option<Posted> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let v = self.slots[tail % CAPACITY].load(Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Posted::decode(v)
    }

    /// Whether a post was dropped since the last call (and forgets it).
    pub fn take_lost(&self) -> bool {
        self.lost.swap(false, Ordering::AcqRel)
    }
}

impl Default for Queue {
    fn default() -> Self {
        Queue::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_effect() -> Vec<Effect> {
        let mut v = vec![Effect::Choose, Effect::Cancel];
        for scope in [Scope::AllWindows, Scope::ThisApp] {
            for backwards in [false, true] {
                v.push(Effect::Open { scope, backwards });
            }
        }
        for backwards in [false, true] {
            v.push(Effect::Step { backwards });
        }
        for d in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            v.push(Effect::Move(d));
        }
        v
    }

    #[test]
    fn every_post_survives_the_trip_through_one_u32() {
        assert_eq!(Posted::decode(Posted::StartTap.encode()), Some(Posted::StartTap));
        assert_eq!(Posted::decode(Posted::Lost.encode()), Some(Posted::Lost));
        assert_eq!(Posted::decode(Posted::WinCombo(Combo::Explorer).encode()), Some(Posted::WinCombo(Combo::Explorer)));
        assert_eq!(Posted::decode(Posted::WinCombo(Combo::Desktop).encode()), Some(Posted::WinCombo(Combo::Desktop)));
        for effect in every_effect() {
            for seq in [0u16, 1, 2, 40_000, u16::MAX] {
                let p = Posted::Chord { effect, seq };
                assert_eq!(Posted::decode(p.encode()), Some(p), "{p:?}");
            }
        }
        assert_eq!(Posted::decode(0), None);
        assert_eq!(Posted::decode(0x3F), None, "a code nothing posts");
    }

    #[test]
    fn posts_come_out_in_the_order_they_went_in_and_the_ring_wraps() {
        let q = Queue::new();
        assert_eq!(q.pop(), None);
        for round in 0..5u16 {
            for i in 0..40u16 {
                q.push(Posted::Chord { effect: Effect::Step { backwards: false }, seq: round * 100 + i });
            }
            for i in 0..40u16 {
                assert_eq!(q.pop(), Some(Posted::Chord { effect: Effect::Step { backwards: false }, seq: round * 100 + i }));
            }
            assert_eq!(q.pop(), None);
        }
        assert!(!q.take_lost());
    }

    #[test]
    fn a_full_ring_drops_the_post_raises_the_flag_and_never_blocks() {
        let q = Queue::new();
        for _ in 0..CAPACITY {
            q.push(Posted::StartTap);
        }
        assert!(!q.take_lost());
        q.push(Posted::StartTap); // the 65th: dropped
        assert!(q.take_lost());
        assert!(!q.take_lost(), "the flag is read once");
        let mut n = 0;
        while q.pop().is_some() {
            n += 1;
        }
        assert_eq!(n, CAPACITY);
        // room again
        q.push(Posted::Lost);
        assert_eq!(q.pop(), Some(Posted::Lost));
    }

    #[test]
    fn one_writer_thread_and_one_reader_thread_lose_nothing() {
        use std::sync::Arc;
        let q = Arc::new(Queue::new());
        let w = q.clone();
        let total = 5_000u16;
        let writer = std::thread::spawn(move || {
            let mut sent = 0u16;
            while sent < total {
                // (the writer never waits for the reader; it retries only because this test wants every post delivered)
                w.push(Posted::Chord { effect: Effect::Choose, seq: sent });
                if w.take_lost() {
                    std::thread::yield_now();
                    continue;
                }
                sent += 1;
            }
        });
        let mut next = 0u16;
        while next < total {
            match q.pop() {
                Some(Posted::Chord { seq, .. }) => {
                    assert_eq!(seq, next, "in order, none twice");
                    next += 1;
                }
                Some(_) => unreachable!(),
                None => std::thread::yield_now(),
            }
        }
        writer.join().unwrap();
    }
}
