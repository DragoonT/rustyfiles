//! What the dock's control centre (and the right island's network and volume symbols) read from Windows: sound, the system media
//! session, the network, Bluetooth, the battery and the monitor's brightness. No window, no UI: each source is a module with
//!
//! * a SNAPSHOT: plain data (`Clone + PartialEq`, no handles), what the interface draws from;
//! * a way to READ it once (blocking: a worker thread, never the UI thread);
//! * a way to FOLLOW it: the callback is called from the source's own thread when the system says something changed. These are
//!   events from the system (COM and WinRT callbacks), never a polling loop. Where Windows offers no event it is said in that
//!   module and the poll is the caller's, no faster than once in 5 s and only while the panel is open.
//!
//! | module | source | events | runs |
//! |---|---|---|---|
//! | `audio` | Core Audio: the default output's volume, mute, name and the list of outputs | `IAudioEndpointVolumeCallback`, `IMMNotificationClient` | a thread of its own (COM MTA), from the first `subscribe`, until the process ends: the island shows it all the time |
//! | `network` | WinRT `NetworkInformation`: Ethernet / Wi-Fi (name, bars) / not connected | `NetworkStatusChanged` (no event for a Wi-Fi signal that only gets stronger or weaker: the panel asks again every 5 s while open) | a thread of its own (MTA), for the life of the process: the island shows it all the time |
//! | `media` | WinRT `GlobalSystemMediaTransportControlsSessionManager` | sessions, current session, playback info, media properties | a thread of its own while the panel is open |
//! | `bluetooth` | WinRT `Radio` (on / off) and the connected devices | `Radio.StateChanged`, a `DeviceWatcher` | a thread of its own while the panel is open |
//! | `power` | `GetSystemPowerStatus`: battery percent, charging | `RegisterPowerSettingNotification` (callback form, no window) | nothing of our own: Windows calls back |
//! | `brightness` | DDC/CI of the primary monitor (`dxva2`) | none (a monitor's own buttons cannot be heard): read when the panel opens | a thread of its own while the panel is open |
//!
//! Nothing here changes a setting by itself: the writes (`audio::set_volume`, `audio::set_muted`, `media::Transport`,
//! `brightness::Brightness::set`) are called only as the direct result of the user's click or drag in the panel.
//!
//! COM: the audio, WinRT and brightness threads are initialised as MTA (what these APIs and their callbacks want). None of them is
//! the shell thread (`crate::sta`), and none is a single-threaded apartment that does not pump messages.

pub mod audio;
pub mod bluetooth;
pub mod brightness;
pub mod media;
pub mod network;
pub mod power;

use std::sync::Arc;

use parking_lot::Mutex;

/// The latest value of a source and the callbacks that follow it. Cloning gives another handle to the same hub.
pub struct Hub<T>(Arc<HubShared<T>>);

struct HubShared<T> {
    inner: Mutex<HubInner<T>>,
}

/// A follower of a hub.
type Follower<T> = Arc<dyn Fn(&T) + Send + Sync>;

struct HubInner<T> {
    latest: Option<T>,
    subs: Vec<(u64, Follower<T>)>,
    next: u64,
}

impl<T> Clone for Hub<T> {
    fn clone(&self) -> Self {
        Hub(self.0.clone())
    }
}

impl<T: Clone + PartialEq + Send + 'static> Hub<T> {
    pub fn new() -> Hub<T> {
        Hub(Arc::new(HubShared { inner: Mutex::new(HubInner { latest: None, subs: Vec::new(), next: 1 }) }))
    }

    /// Stores `value` and tells every subscriber, unless it is what they were told last (a burst of events that changes nothing
    /// reaches nobody). The callbacks run on the calling thread, outside the lock.
    pub fn publish(&self, value: T) {
        let subs: Vec<Follower<T>> = {
            let mut g = self.0.inner.lock();
            if g.latest.as_ref() == Some(&value) {
                return;
            }
            g.latest = Some(value.clone());
            g.subs.iter().map(|(_, f)| f.clone()).collect()
        };
        for f in subs {
            f(&value);
        }
    }

    /// What was published last (`None` before the first reading).
    pub fn latest(&self) -> Option<T> {
        self.0.inner.lock().latest.clone()
    }

    /// Follows the hub: `callback` is called with the latest value at once (when there is one) and at every change after it, from
    /// the source's thread. Dropping the returned guard ends it.
    pub fn subscribe(&self, callback: impl Fn(&T) + Send + Sync + 'static) -> Subscription {
        let f: Follower<T> = Arc::new(callback);
        let (id, now) = {
            let mut g = self.0.inner.lock();
            let id = g.next;
            g.next += 1;
            g.subs.push((id, f.clone()));
            (id, g.latest.clone())
        };
        if let Some(v) = now {
            f(&v);
        }
        let hub = self.clone();
        Subscription { end: Some(Box::new(move || hub.0.inner.lock().subs.retain(|(i, _)| *i != id))) }
    }
}

impl<T: Clone + PartialEq + Send + 'static> Default for Hub<T> {
    fn default() -> Self {
        Hub::new()
    }
}

/// Following a source. Dropping it stops the callbacks (and, for a source that runs a thread of its own for its followers, that
/// thread).
#[must_use = "dropping a Subscription ends it"]
pub struct Subscription {
    end: Option<Box<dyn FnOnce() + Send>>,
}

impl Subscription {
    pub(crate) fn new(end: impl FnOnce() + Send + 'static) -> Subscription {
        Subscription { end: Some(Box::new(end)) }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(f) = self.end.take() {
            f();
        }
    }
}

/// Writes that arrive faster than they can be made, kept to the last one. A slider sends a value for every pixel of a drag; the
/// call that applies it (a COM call, a DDC/CI transfer) takes longer than that, so only the newest value is worth sending. The
/// producer calls [`push`](Latest::push) (which says whether the worker has to be woken), the worker calls [`take`](Latest::take).
#[derive(Debug)]
pub struct Latest<T> {
    pending: Option<T>,
    woken: bool,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Latest { pending: None, woken: false }
    }
}

impl<T> Latest<T> {
    /// Keeps `value` in place of any that was waiting. True when the worker has to be woken (it has not been since its last take).
    pub fn push(&mut self, value: T) -> bool {
        self.pending = Some(value);
        !std::mem::replace(&mut self.woken, true)
    }

    /// The newest value, and the worker may be woken again.
    pub fn take(&mut self) -> Option<T> {
        self.woken = false;
        self.pending.take()
    }
}

/// A [`Latest`] shared between the thread that pushes and the thread that takes.
pub(crate) struct Shared<T>(Mutex<Latest<T>>);

impl<T> Shared<T> {
    pub fn new() -> Arc<Shared<T>> {
        Arc::new(Shared(Mutex::new(Latest::default())))
    }

    pub fn push(&self, v: T) -> bool {
        self.0.lock().push(v)
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().take()
    }
}

/// Starts COM on the calling thread as a multithreaded apartment (what the audio and WinRT APIs want). Harmless when it already is.
pub(crate) fn init_mta() {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_hub_tells_its_followers_only_what_changed() {
        let hub: Hub<u32> = Hub::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let _sub = hub.subscribe(move |v| s.lock().push(*v));
        assert!(seen.lock().is_empty(), "nothing has been read yet");
        hub.publish(5);
        hub.publish(5);
        hub.publish(6);
        assert_eq!(*seen.lock(), vec![5, 6], "the same value twice reaches nobody");
        assert_eq!(hub.latest(), Some(6));
    }

    #[test]
    fn a_follower_that_comes_late_is_told_the_latest_at_once_and_one_that_left_hears_no_more() {
        let hub: Hub<u32> = Hub::new();
        hub.publish(1);
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let sub = hub.subscribe(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 1, "told the current value on joining");
        hub.publish(2);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        drop(sub);
        hub.publish(3);
        assert_eq!(count.load(Ordering::SeqCst), 2, "dropped: no more calls");
    }

    #[test]
    fn writes_that_pile_up_are_cut_to_the_last_and_the_worker_is_woken_once() {
        let mut q: Latest<u32> = Latest::default();
        // a drag sends a hundred values before the worker gets to the first
        let wakes = (1..=100).filter(|&v| q.push(v)).count();
        assert_eq!(wakes, 1, "one wake-up for the whole burst");
        assert_eq!(q.take(), Some(100), "only the newest is applied");
        assert_eq!(q.take(), None, "and it is applied once");
        // after a take the next write wakes the worker again
        assert!(q.push(7));
        assert!(!q.push(8));
        assert_eq!(q.take(), Some(8));
    }

    #[test]
    fn the_shared_queue_is_the_same_thing_between_threads() {
        let q = Shared::<u32>::new();
        let q2 = q.clone();
        let woke = std::thread::spawn(move || (0..50).filter(|&v| q2.push(v)).count()).join().unwrap();
        assert_eq!(woke, 1);
        assert_eq!(q.take(), Some(49));
    }
}
