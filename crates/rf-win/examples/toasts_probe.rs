//! Prints COUNTS about the notification list and the calendar's inputs on this machine, for the dock's bell and clock panels.
//! Read-only and windowless: it never calls the remove or clear calls, never the call that asks for consent, never prints a title, a
//! body or a program's name (only numbers), and writes no file.
//!
//!     cargo run -p rf-win --example toasts_probe --offline            (one reading, two polls, a short dot test)
//!     cargo run -p rf-win --example toasts_probe --offline -- 30      (also counts the events of the notification folder for 30 s)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rf_win::dock::calendar;
use rf_win::toasts::{self, Access, Ago};

fn main() {
    let watch_secs: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(0);

    println!("-- calendar inputs");
    let first = calendar::first_day_of_week();
    let today = calendar::today();
    let grid = calendar::month_grid(today.year, today.month, first, Some(today));
    println!(
        "first day of the week: {first} (0 = Monday)  today is weekday {}  grid: {} rows x {} columns, {} days in the month, {} marked today",
        today.weekday(),
        grid.len(),
        grid[0].len(),
        grid.iter().flatten().filter(|c| c.in_month).count(),
        grid.iter().flatten().filter(|c| c.today).count()
    );
    println!("long time pattern has seconds: {}", calendar::long_time_pattern().contains('s'));

    println!("-- the listener (one reading, on the reader thread)");
    let (tx, rx) = channel();
    let started = Instant::now();
    let w = toasts::listener::watch(toasts::POLL_EVERY, move |s| {
        let _ = tx.send(s.clone());
    });
    let first_snap = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(s) => s,
        Err(_) => {
            println!("NO reading within 10 s");
            return;
        }
    };
    println!("first callback after {} ms: access {:?}, {} notifications", started.elapsed().as_millis(), first_snap.access, first_snap.toasts.len());
    if first_snap.access == Access::Granted {
        let groups = toasts::group(&first_snap.toasts);
        let now = toasts::now_unix();
        let today_day = toasts::local_day(now);
        let (mut now_n, mut min_n, mut hour_n, mut yest_n, mut date_n) = (0, 0, 0, 0, 0);
        for t in &first_snap.toasts {
            match toasts::ago(now - t.created, today_day, toasts::local_day(t.created)) {
                Ago::Now => now_n += 1,
                Ago::Minutes(_) => min_n += 1,
                Ago::Hours(_) => hour_n += 1,
                Ago::Yesterday => yest_n += 1,
                Ago::Date => date_n += 1,
            }
        }
        println!(
            "groups: {}  with an app id: {}  with a title: {}  with a body: {}  body lines after the clamp, at most: {}",
            groups.len(),
            first_snap.toasts.iter().filter(|t| !t.app_id.is_empty()).count(),
            first_snap.toasts.iter().filter(|t| !t.title.is_empty()).count(),
            first_snap.toasts.iter().filter(|t| !t.body.is_empty()).count(),
            first_snap.toasts.iter().map(|t| toasts::body_lines(&t.body).lines().count()).max().unwrap_or(0),
        );
        println!("ages: now {now_n}, minutes {min_n}, hours {hour_n}, yesterday {yest_n}, older {date_n}");
        println!("draw order has {} ids, all different: {}", toasts::draw_order(&first_snap.toasts, &groups).len(), {
            let mut o = toasts::draw_order(&first_snap.toasts, &groups);
            o.sort_unstable();
            o.dedup();
            o.len() == first_snap.toasts.len()
        });
    } else {
        println!("(access is not granted: the panel would show its one line and the button to {})", toasts::PRIVACY_PAGE);
    }

    println!("-- two polls apart: does the list flicker?");
    w.refresh();
    // a second reading that differs from the first is told; one that is the same is not (that is the point)
    match rx.recv_timeout(toasts::POLL_EVERY + Duration::from_secs(2)) {
        Ok(second) => {
            let d = toasts::diff(&first_snap.toasts, &second.toasts);
            println!("the list changed between two polls: {} added, {} removed, {} changed", d.added.len(), d.removed.len(), d.changed.len());
        }
        Err(_) => println!("no second callback within {} s: the readings were the same, so there was nothing to redraw", toasts::POLL_EVERY.as_secs() + 2),
    }
    drop(w);

    println!("-- the dot's worker (one count now; then, if asked, the events of the folder)");
    let dir = toasts::hint::notifications_dir();
    println!("notification folder exists: {}", dir.as_ref().is_some_and(|d| d.is_dir()));
    let (tx, rx) = channel();
    let dot = toasts::hint::start(move |c| {
        let _ = tx.send(c);
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(c) => println!("first count: access {:?}, {} notifications, the folder is watched: {}", c.access, c.count, dot.hinted()),
        Err(_) => println!("no first count within 10 s"),
    }
    if watch_secs > 0 {
        if let Some(d) = dir.filter(|d| d.is_dir()) {
            let events = Arc::new(AtomicUsize::new(0));
            let e = events.clone();
            // (this only counts the folder's events; it reads nothing)
            let watch = rf_win::watch::watch_dir(&d, Duration::from_millis(300), move || {
                e.fetch_add(1, Ordering::SeqCst);
            });
            if watch.is_ok() {
                println!("counting the folder's change bursts for {watch_secs} s (nothing is done with them) ...");
                std::thread::sleep(Duration::from_secs(watch_secs));
                println!("change bursts seen in {watch_secs} s: {}", events.load(Ordering::SeqCst));
            } else {
                println!("the folder cannot be watched");
            }
        }
        // anything the dot's worker learned meanwhile
        let mut later = 0;
        while rx.try_recv().is_ok() {
            later += 1;
        }
        println!("count changes the dot told meanwhile: {later}");
    }
    drop(dot);
    std::thread::sleep(Duration::from_millis(300));
    println!("done");
}
