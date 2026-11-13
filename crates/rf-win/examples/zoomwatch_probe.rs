//! Windowless, read-only probe of the thin dock's two system parts: what the placement watch costs on a live desktop and how long the
//! walk that answers "is a maximised window on the dock's monitor" takes. No window is made, nothing is moved, no input is sent.
//!
//!     cargo run --offline -p rf-win --example zoomwatch_probe [seconds]
//!
//! Prints, for the length of the run: the events the hook function saw (all of them, those for a whole window, those that needed a
//! style read, those passed on, those told after the coalescing) per second, and this process's CPU time (the hook thread's cost is
//! in it; the probe does nothing else while it waits). Then the walk's time over 300 reads and the cursor read's time. Counts only:
//! no window title, name or path is read or printed.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

fn cpu_ms() -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
    ticks(k) + ticks(u)
}

fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let mon = rf_win::dock::primary_monitor().expect("a primary monitor");
    println!("primary monitor {}x{} at {} dpi", mon.rect.width(), mon.rect.height(), mon.dpi);

    let told = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let t2 = told.clone();
    let watch = rf_win::dock::zoomwatch::ZoomWatch::start(move || {
        t2.fetch_add(1, Ordering::Relaxed);
    })
    .expect("the watch starts");
    let stats = watch.stats();
    let (cpu0, t0) = (cpu_ms(), Instant::now());
    std::thread::sleep(Duration::from_secs(secs));
    let (cpu1, dt) = (cpu_ms(), t0.elapsed().as_secs_f64());
    let n = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed) as f64;
    println!("over {dt:.1} s (the user's own use of the desktop; the probe does nothing meanwhile):");
    println!("  hook calls               {:>8.0}  {:>7.1} per second", n(&stats.seen), n(&stats.seen) / dt);
    println!("  for a whole window       {:>8.0}  {:>7.1} per second", n(&stats.windows), n(&stats.windows) / dt);
    println!("  needed a style read      {:>8.0}  {:>7.1} per second", n(&stats.style_reads), n(&stats.style_reads) / dt);
    println!("  passed on                {:>8.0}  {:>7.2} per second", n(&stats.passed), n(&stats.passed) / dt);
    println!("  told after coalescing    {:>8.0}  {:>7.2} per second  (callback count {})", n(&stats.told), n(&stats.told) / dt, told.load(Ordering::Relaxed));
    println!("  this process's CPU       {:.1} ms in {dt:.1} s = {:.3} % of one core", cpu1 - cpu0, (cpu1 - cpu0) / (dt * 1000.0) * 100.0);
    drop(watch);

    // the walk
    let reads = 300;
    let t = Instant::now();
    let mut yes = 0;
    for _ in 0..reads {
        if rf_win::dock::zoomwatch::maximized_on(mon.handle) {
            yes += 1;
        }
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / reads as f64;
    println!("the walk over the windows: {per:.0} us per read ({reads} reads; a maximised window was seen in {yes} of them)");
    let t = Instant::now();
    for _ in 0..10_000 {
        let _ = rf_win::dock::zoomwatch::pointer();
    }
    println!("the cursor read (position and buttons): {:.2} us per read", t.elapsed().as_secs_f64() * 1e6 / 10_000.0);
}
