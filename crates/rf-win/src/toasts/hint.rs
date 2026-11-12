//! The bell's dot while the panel is closed: the number of notifications, kept fresh by ONE cheap signal.
//!
//! THE SIGNAL is a plain directory-change wait (`ReadDirectoryChangesW`, through `crate::watch`) on Windows' notification folder,
//! `%LOCALAPPDATA%\Microsoft\Windows\Notifications`, where the notification database lives. A change there is only a HINT that the
//! list may have changed: the worker then waits for the folder to settle ([`super::HINT_SETTLE`]), keeps at least
//! [`super::HINT_MIN_GAP`] between two reads (the rule is [`super::read_at`], kept by [`HintTimer`]), and counts the list once. Nothing polls: with no change in the folder the thread
//! sleeps in a channel and the only other wake-up is the panel closing. While the bell's panel is open the worker is paused ([`Dot::pause`]): the
//! panel reads the list itself every 2 s, and a second read of the same list for a hint would only be waste. That every new notification touches the folder is an
//! observation of the research and is not guaranteed by Windows; if the folder cannot be watched ([`Dot::hinted`] is false) the dot is only as
//! fresh as the last time the panel was open, and the panel says what it saw when it closes ([`Dot::set`]).
//!
//! Only a COUNT is kept: the notifications read for it are dropped on the spot, with their texts.

use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use super::{Access, HintTimer};
use crate::sysstatus::{Subscription, init_mta};
use crate::watch::DirWatch;

/// What the dot knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DotCount {
    pub access: Access,
    pub count: usize,
}

enum Msg {
    /// The folder changed.
    Hint,
    /// The panel closed and saw this many: take it as the count (no read).
    Set(DotCount),
    /// The panel opened (true) or went away (false).
    Pause(bool),
    Quit,
}

/// The folder Windows keeps the notification database in.
pub fn notifications_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"Microsoft\Windows\Notifications"))
}

/// The dot's worker. Dropping it ends the thread and the folder watch.
pub struct Dot {
    tx: Sender<Msg>,
    hinted: bool,
    _watch: Option<DirWatch>,
    _end: Subscription,
}

impl Dot {
    /// Whether the folder is being watched (else the count only changes when [`set`](Dot::set) says so).
    pub fn hinted(&self) -> bool {
        self.hinted
    }

    /// The panel opened (hint reads stop: it reads the list itself) or went away (one read, for what changed meanwhile).
    pub fn pause(&self, paused: bool) {
        let _ = self.tx.send(Msg::Pause(paused));
    }

    /// The panel closed: this is what it last saw.
    pub fn set(&self, c: DotCount) {
        let _ = self.tx.send(Msg::Set(c));
    }
}

/// Starts the worker: one read now, then one per hint. `callback` runs on the worker's thread, only when the count differs from the
/// one told before.
pub fn start(callback: impl Fn(DotCount) + Send + 'static) -> Dot {
    let (tx, rx) = channel::<Msg>();
    let hint_tx = tx.clone();
    // (the debounce here is the folder watcher's own; the worker settles again on top of it)
    let watch = notifications_dir().filter(|d| d.is_dir()).and_then(|dir| {
        crate::watch::watch_dir(&dir, Duration::from_millis(300), move || {
            let _ = hint_tx.send(Msg::Hint);
        })
        .map_err(|e| log::debug!("notifications: the folder cannot be watched ({e}); the dot is as fresh as the last opening"))
        .ok()
    });
    let hinted = watch.is_some();
    let spawned = std::thread::Builder::new().name("rf-toast-dot".into()).spawn(move || {
        init_mta();
        let mut last: Option<DotCount> = None;
        let mut last_read: Option<Instant> = None;
        let tell = |c: DotCount, last: &mut Option<DotCount>| {
            if *last != Some(c) {
                callback(c);
                *last = Some(c);
            }
        };
        let read = |last_read: &mut Option<Instant>| {
            *last_read = Some(Instant::now());
            super::listener::count_now().map(|(access, count)| DotCount { access, count })
        };
        if let Some(c) = read(&mut last_read) {
            tell(c, &mut last);
        }
        // (the timer only decides WHEN; every wait below is the time it asks for, so a second hint inside the wait cannot shorten it)
        let mut timer = HintTimer::default();
        loop {
            let msg = match timer.due(last_read) {
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                },
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
            };
            match msg {
                Some(Msg::Quit) => break,
                Some(Msg::Set(c)) => tell(c, &mut last),
                Some(Msg::Hint) => timer.hint(Instant::now()),
                Some(Msg::Pause(p)) => timer.set_paused(p, Instant::now()),
                None => {
                    timer.read_done();
                    if let Some(c) = read(&mut last_read) {
                        tell(c, &mut last);
                    }
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("notifications: the dot's thread did not start: {e}");
    }
    let quit = tx.clone();
    Dot {
        tx,
        hinted,
        _watch: watch,
        _end: Subscription::new(move || {
            let _ = quit.send(Msg::Quit);
        }),
    }
}
