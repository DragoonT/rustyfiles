//! The system side of the desktop that rustyfiles draws itself (shell plan increments F1 and F2). The rules and the data live in
//! `rf_core::desktop` and are tested there on plain data; this module reads what Windows holds and hands it over as that data.
//! EVERYTHING HERE IS READ-ONLY: nothing writes the registry, the wallpaper setting, the Desktop folders or Explorer's state.
//!
//! | file | what it is | unit-tested with |
//! |---|---|---|
//! | `registry` | the real read-only registry (`RegRead`) | the in-memory fake in rf-core |
//! | `monitors` | the connected monitors: rectangles, work areas, scale, stable device path | - (system) |
//! | `regnotify` | `RegNotifyChangeKeyValue` on a list of keys, one thread, stops on drop | start / stop only (a change would need a write) |
//! | `wallpaper` | the wallpaper: `IDesktopWallpaper` on the shell thread, the registry as fallback | the API behind a trait with a fake |
//! | `render` | decode the picture once, cut / scale / tile it to one bitmap per monitor | synthetic pictures, exact pixels |
//! | `wallcache` | the on-disk cache of those bitmaps | temp folder |
//! | `wallwatch` | what tells us the wallpaper changed, debounced, compared with the last read | fake event source and reader |
//! | `items` | the Desktop and Public Desktop folders and the virtual items, kept up to date by `watch.rs` | temp folders |
//! | `import` | the one-time read of Explorer's icon positions (`IFolderView::GetItemPosition`) | the name classification |
//! | `cover` | cover mode (F4): turns a window into one of the desktop's per-monitor cover windows and keeps it directly above Explorer's desktop (writes nothing of Explorer's; moves only our own windows) | styles, a fake-free read of the z-order; the rules are in `rf_core::desktop::cover` |
//! | `bare` | the cover windows when Explorer's desktop leaves and comes back: placed again at once (moves only our own windows) | a fake z-order |
//! | `showdesktop` | Show Desktop / Minimize All carried out over a trait | a fake desktop |
//!
//! Every shell call that is not a plain file call goes through [`crate::sta::call`] (workers that call the shell themselves hang it:
//! see the comment there). Every function may block; none belongs on the UI thread.

pub mod bare;
pub mod cover;
pub mod import;
pub mod items;
pub mod monitors;
pub mod regnotify;
pub mod registry;
pub mod render;
pub mod showdesktop;
pub mod wallcache;
pub mod wallpaper;
pub mod wallwatch;

/// The longest the shell thread is waited for in this module.
pub(crate) const SHELL_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Waits until `quiet` passes with nothing arriving on `rx`, but never longer than `max` from now: a source that keeps writing at
/// less than the quiet interval (a slideshow, a chatty tool) must not hold a reader off for ever. `false`: the sender is gone.
pub(crate) fn settle<T>(rx: &crossbeam_channel::Receiver<T>, quiet: std::time::Duration, max: std::time::Duration) -> bool {
    use crossbeam_channel::RecvTimeoutError;
    let start = std::time::Instant::now();
    loop {
        let left = max.saturating_sub(start.elapsed());
        if left.is_zero() {
            return true;
        }
        match rx.recv_timeout(quiet.min(left)) {
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// Runs a job once after a burst of pokes: `quiet` after the last poke, or `max` after the first one, whichever comes first. The
/// thread ends when the `Coalescer` is dropped.
pub(crate) struct Coalescer {
    tx: crossbeam_channel::Sender<()>,
}

impl Coalescer {
    pub(crate) fn start(quiet: std::time::Duration, max: std::time::Duration, job: impl Fn() + Send + 'static) -> Coalescer {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        let _ = std::thread::Builder::new().name("rf-desktop-coalesce".into()).spawn(move || {
            while rx.recv().is_ok() {
                if !settle(&rx, quiet, max) {
                    return;
                }
                job();
            }
        });
        Coalescer { tx }
    }
    pub(crate) fn poke(&self) {
        let _ = self.tx.send(());
    }
}

/// Why a guarded shell call gave no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    /// An earlier call of this kind has not come back yet (it may be stuck in COM activation): this one was not even queued.
    Busy,
    /// The call was queued and did not finish in time. It keeps running there; the gate stays closed until it ends.
    TimedOut,
}

/// Lets one COM-activating job at a time into the shell thread. `sta::call`'s limit only stops the CALLER waiting; the job runs on,
/// and everything queued behind a job stuck in `CoCreateInstance` (the dock's icons among it) waits with it. With the gate, a
/// second call while the first has not ended returns [`GateError::Busy`] at once and adds nothing to the queue, so one hung server
/// costs one stuck job and not a growing line of them.
pub struct Gate(std::sync::atomic::AtomicBool);

impl Gate {
    pub const fn new() -> Self {
        Gate(std::sync::atomic::AtomicBool::new(false))
    }

    pub fn call<R: Send + 'static>(&'static self, limit: std::time::Duration, f: impl FnOnce() -> R + Send + 'static) -> Result<R, GateError> {
        use std::sync::atomic::Ordering;
        if self.0.swap(true, Ordering::AcqRel) {
            return Err(GateError::Busy);
        }
        struct Open(&'static Gate);
        impl Drop for Open {
            fn drop(&mut self) {
                self.0.0.store(false, Ordering::Release);
            }
        }
        let guard = Open(self);
        match crate::sta::call(limit, move || {
            let _open = guard; // opened when the job ends, however it ends
            f()
        }) {
            Some(r) => Ok(r),
            None => Err(GateError::TimedOut),
        }
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

/// The one gate for everything in this module that makes the shell thread activate a COM class whose server may not be there
/// (`IDesktopWallpaper` with no Explorer is shell plan test T3, not yet run; `ShellWindows` likewise).
pub(crate) static COM_ACTIVATION: Gate = Gate::new();

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn settle_waits_for_quiet_but_not_for_ever() {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        // a steady stream, one event every 20 ms for 1.5 s, with a quiet time of 80 ms: only the cap ends the wait
        let sender = std::thread::spawn(move || {
            for _ in 0..75 {
                if tx.send(()).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let t = Instant::now();
        assert!(settle(&rx, Duration::from_millis(80), Duration::from_millis(400)));
        let waited = t.elapsed();
        assert!(waited >= Duration::from_millis(390) && waited < Duration::from_millis(900), "ended by the cap: {waited:?}");
        drop(rx);
        sender.join().unwrap();
        // without a stream the quiet time ends it, long before the cap
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        let t = Instant::now();
        assert!(settle(&rx, Duration::from_millis(50), Duration::from_secs(10)));
        assert!(t.elapsed() < Duration::from_secs(2));
        drop(tx);
        assert!(!settle(&rx, Duration::from_millis(50), Duration::from_secs(10)), "a gone sender is reported");
    }

    #[test]
    fn a_burst_of_pokes_runs_the_job_once_and_a_steady_stream_still_runs_it() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let runs = Arc::new(AtomicUsize::new(0));
        let r2 = runs.clone();
        let co = Coalescer::start(Duration::from_millis(80), Duration::from_millis(400), move || {
            r2.fetch_add(1, Ordering::SeqCst);
        });
        for _ in 0..10 {
            co.poke();
        }
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "ten pokes in a burst, one run");
        // pokes every 30 ms for a second: the job runs while they continue
        let stop = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < stop {
            co.poke();
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(runs.load(Ordering::SeqCst) >= 3, "the cap made it run during the stream: {}", runs.load(Ordering::SeqCst));
        drop(co);
    }

    #[test]
    fn the_gate_lets_one_job_in_and_turns_the_others_away_without_queueing_them() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static GATE: Gate = Gate::new();
        static RAN: AtomicUsize = AtomicUsize::new(0);
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        // the first call gets stuck (like a CoCreateInstance that never returns) and the caller gives up
        let first = GATE.call(Duration::from_millis(100), move || {
            RAN.fetch_add(1, Ordering::SeqCst);
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        });
        assert_eq!(first, Err(GateError::TimedOut));
        // while it is stuck, others are turned away at once and never reach the shell thread
        let t = Instant::now();
        let second = GATE.call(Duration::from_secs(5), || RAN.fetch_add(1, Ordering::SeqCst));
        assert_eq!(second, Err(GateError::Busy));
        assert!(t.elapsed() < Duration::from_millis(500));
        // the stuck job ends: the gate opens and the next call is answered
        release_tx.send(()).unwrap();
        let mut answer = Err(GateError::Busy);
        for _ in 0..100 {
            answer = GATE.call(Duration::from_secs(5), || 7);
            if answer != Err(GateError::Busy) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(answer, Ok(7));
        assert_eq!(RAN.load(Ordering::SeqCst), 1, "the turned-away job never ran");
    }
}
