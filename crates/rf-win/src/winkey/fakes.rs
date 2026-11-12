//! Stand-ins for the parts of the layer that touch the system, for tests and for the windowless probe: a mask sender that counts, a
//! keyboard probe whose keys the test presses, and a feed that cannot be installed. They send nothing and read nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

use super::mask::MaskSender;
use super::service::KeyProbe;
use crate::switcher::hook::{KeyFeed, OnRaw};

/// Counts the masks it was asked to send.
#[derive(Default)]
pub struct FakeMask {
    sent: AtomicU32,
    /// Makes `send` answer false (the system refusing the input).
    pub refuse: std::sync::atomic::AtomicBool,
}

impl FakeMask {
    pub fn sent(&self) -> u32 {
        self.sent.load(Ordering::Relaxed)
    }
}

impl MaskSender for FakeMask {
    fn send(&self) -> bool {
        self.sent.fetch_add(1, Ordering::Relaxed);
        !self.refuse.load(Ordering::Relaxed)
    }
}

/// A keyboard whose pressed keys the test sets.
#[derive(Default)]
pub struct FakeProbe {
    down: Mutex<HashSet<u32>>,
}

impl FakeProbe {
    pub fn set(&self, vk: u32, down: bool) {
        let mut d = self.down.lock();
        if down {
            d.insert(vk);
        } else {
            d.remove(&vk);
        }
    }
}

impl KeyProbe for FakeProbe {
    fn down(&self, vk: u32) -> bool {
        self.down.lock().contains(&vk)
    }
}

/// A feed that cannot be started (the system refused the hook).
#[derive(Default)]
pub struct RefusingFeed {
    pub attempts: u32,
}

impl KeyFeed for RefusingFeed {
    fn start_raw(&mut self, _on_raw: OnRaw) -> anyhow::Result<()> {
        self.attempts += 1;
        anyhow::bail!("the system refused the hook")
    }
    fn stop(&mut self) {}
    fn running(&self) -> bool {
        false
    }
}
