//! What every dock shares about the windows: ONE watch of the task list and ONE listing of the running windows, however many monitors have
//! a dock.
//!
//! With a dock on every monitor each middle island used to start its own `tasklist::watch` (two threads and three WinEvent hooks) and to
//! list the windows itself (`identity::running_groups` opens a handle to every program that has a window), so every window event was
//! watched twice and swept twice. Now the watch is started once, by the first island, and tells every island that listens; and the
//! listing is made once for the islands that ask while no window event has come in between (the second island gets the first one's
//! answer). What stays per dock is only what is drawn, laid out and pointed at: each island applies its own monitor's rule
//! (`rf_win::dock::monitors::filter_groups`) to the shared listing.
//!
//! With ONE dock nothing is kept: the listing is made at every request, as before, and the only addition is a lock.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use rf_win::tasklist::{self, Changes, Task, TaskWatch};

/// The windows as the dock groups them (`rf_win::dock::identity::running_groups`).
pub type Groups = Vec<(String, Vec<Task>)>;

/// A listing is reused for at most this long even when no event came: a watch that failed to start would otherwise leave it stale for
/// ever, and an island that asks again a second later (a pin list that changed, a rule that changed) wants what the windows are now.
const FRESH: Duration = Duration::from_secs(1);

struct Feed {
    listeners: Vec<UnboundedSender<()>>,
    /// Kept for as long as the process lives: the watch stops when this is dropped.
    watch: Option<TaskWatch>,
}

static FEED: Mutex<Feed> = Mutex::new(Feed { listeners: Vec::new(), watch: None });
/// Counts the task-list events (a window came, went, was shown, hidden, minimised, restored or took the front).
static EVENTS: AtomicU64 = AtomicU64::new(0);

struct Listing {
    events: u64,
    at: Instant,
    groups: Groups,
}

static LISTING: Mutex<Option<Listing>> = Mutex::new(None);

/// An island asks to be told (with one unit on `tx`) whenever the task list changes. The first call starts the one watch.
pub fn subscribe(tx: UnboundedSender<()>) {
    let mut feed = FEED.lock().unwrap_or_else(|p| p.into_inner());
    feed.listeners.push(tx);
    if feed.watch.is_none() {
        // (a window that only changes its title is not news: the dock shows no titles)
        feed.watch = tasklist::watch(on_change).map_err(|e| log::warn!("dock: cannot watch the task windows: {e:#}")).ok();
    }
}

/// Called on the watch's background thread, at most once a frame.
fn on_change(changes: Changes) {
    if !(changes.list || changes.active) {
        return;
    }
    EVENTS.fetch_add(1, Ordering::SeqCst);
    {
        let mut feed = FEED.lock().unwrap_or_else(|p| p.into_inner());
        // an island that is gone has dropped its receiver: it drops out of the list here
        feed.listeners.retain(|tx| tx.unbounded_send(()).is_ok());
    }
    // (the thin dock asks whether a maximised window is there; one atomic read when its setting is off)
    super::thin::task_changed();
}

/// Whether a listing made at `at_events` / `age` still answers a request that comes when the counter is `now_events`.
fn reusable(at_events: u64, now_events: u64, age: Duration) -> bool {
    at_events == now_events && age < FRESH
}

/// The running windows, grouped. Blocks (it opens a handle to every program that has a window): a worker thread. With `shared` (there
/// is more than one dock) a listing made after the latest task-list event is given to every island that asks for it, and an island that
/// comes while another is listing waits for that listing instead of making its own. Without it nothing is kept.
pub fn running_groups(shared: bool) -> Groups {
    if !shared {
        LISTING.lock().unwrap_or_else(|p| p.into_inner()).take();
        return rf_win::dock::identity::running_groups();
    }
    // (the lock is held across the listing on purpose: the island that comes second waits and then takes the first one's answer)
    let mut cache = LISTING.lock().unwrap_or_else(|p| p.into_inner());
    // (read before the listing: an event that comes while it runs makes the next request list again)
    let seen = EVENTS.load(Ordering::SeqCst);
    if let Some(l) = cache.as_ref()
        && reusable(l.events, seen, l.at.elapsed())
    {
        return l.groups.clone();
    }
    let groups = rf_win::dock::identity::running_groups();
    *cache = Some(Listing { events: seen, at: Instant::now(), groups: groups.clone() });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_is_reused_only_when_no_event_came_and_it_is_young() {
        assert!(reusable(5, 5, Duration::from_millis(30)), "the second dock takes the first one's answer");
        assert!(!reusable(5, 6, Duration::from_millis(30)), "a window event since: list again");
        assert!(!reusable(5, 5, FRESH), "a watch that never speaks does not keep a listing for ever");
        assert!(!reusable(5, 5, Duration::from_secs(30)));
    }

    #[test]
    fn an_island_that_is_gone_drops_out_of_the_listeners() {
        let (tx_live, mut rx_live) = futures::channel::mpsc::unbounded::<()>();
        let (tx_gone, rx_gone) = futures::channel::mpsc::unbounded::<()>();
        drop(rx_gone);
        let mut listeners = vec![tx_live, tx_gone];
        listeners.retain(|tx| tx.unbounded_send(()).is_ok());
        assert_eq!(listeners.len(), 1);
        assert!(rx_live.try_next().is_ok_and(|m| m.is_some()), "the live one was told");
    }
}
