//! Windowless probe of what the transfer progress surfaces are fed with: a real copy through our
//! engine (limited to a few MB/s so it lasts several seconds, paused in the middle), the byte
//! counts the engine reports, and what the pure rules in `rf_core::transfer` make of them every
//! half second: speed, time left, the ring, the strip and the card. It opens no window; it
//! writes only inside a temp folder it makes and removes.
//!
//!     cargo run --offline -p rf-win --example transfer_probe

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rf_core::transfer::{self as xfer, OpStats, Stage};
use rf_win::fileops::{Tracker, Transfer};

fn main() {
    let base = std::env::temp_dir().join(format!("rf-transfer-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (src, dst) = (base.join("kaynak"), base.join("hedef"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    // 8 files of 4 MB = 32 MB, copied at 8 MB/s: about 4 s, plus the pause
    let mut sources = Vec::new();
    for i in 0..8 {
        let p = src.join(format!("dosya-{i}.bin"));
        std::fs::write(&p, vec![i as u8; 4 << 20]).unwrap();
        sources.push(p);
    }

    let (tx, rx) = std::sync::mpsc::channel::<(u32, u32)>();
    let tracker = Tracker::new(
        Box::new(move |t, d, _| {
            let _ = tx.send((t, d));
        }),
        Arc::new(AtomicBool::new(false)),
    );
    tracker.limit.store(8 << 20, Ordering::Relaxed);
    let paused = tracker.paused.clone();
    let bytes = tracker.bytes.clone();
    let tracker = Arc::new(tracker);
    let started = Instant::now();
    let worker = {
        let (sources, dst, tracker) = (sources.clone(), dst.clone(), tracker.clone());
        std::thread::spawn(move || rf_win::fastcopy::transfer(Transfer::Copy, &sources, &dst, tracker).unwrap())
    };

    let mut stats = OpStats::default();
    let (mut units, mut was_paused) = ((0u32, 0u32), false);
    let mut next_print = 0.0;
    println!("{:>5}  {:>6}  {:>9}  {:>10}  {:>7}  {:<16}  {:<9}  {}", "t", "done", "bytes", "speed", "eta", "ring", "card", "strip fits (text/speed/eta/buttons)");
    loop {
        let now = started.elapsed().as_secs_f64();
        // the pause: from 1.5 s to 3.0 s
        let want_pause = (1.5..3.0).contains(&now);
        if want_pause != was_paused {
            paused.store(want_pause, Ordering::Relaxed);
            if want_pause {
                stats.pause();
            } else {
                stats.resume(now);
            }
            was_paused = want_pause;
            println!("       -- {} --", if want_pause { "paused" } else { "resumed" });
        }
        while let Ok((t, d)) = rx.try_recv() {
            if t > 0 {
                units = (d, t);
                stats.update(started.elapsed().as_secs_f64(), units, bytes.get());
            }
        }
        let finished = worker.is_finished();
        if now >= next_print || finished {
            next_print = now + 0.5;
            let b = bytes.get();
            let pace = if was_paused { Default::default() } else { stats.pace(now, units, b) };
            let frac = (units.1 > 0).then(|| units.0 as f32 / units.1 as f32);
            let ring = xfer::ring(!finished, frac, finished.then_some(0), true, (now * 1000.0) as u64);
            let ring = match ring {
                xfer::Ring::Arc { steps, alpha } => format!("arc {steps}/{} a={alpha:.1}", xfer::RING_STEPS),
                xfer::Ring::Spin { start } => format!("spin from {start}"),
                other => format!("{other:?}"),
            };
            let card = xfer::card_stage(&xfer::CardIn { outcome: finished.then_some(xfer::Outcome::Done), ran_ms: (now * 1000.0) as u64, since_end_ms: 0, asking: false, dismissed: false });
            let card = match card {
                Stage::Hidden => "hidden".to_string(),
                other => format!("{other:?}"),
            };
            let fit = xfer::strip_fit(560.0, 0);
            println!(
                "{now:>5.1}  {:>5.0}%  {:>4}/{:<4}  {:>10}  {:>7}  {ring:<16}  {card:<9}  {}/{}/{}/{}",
                frac.unwrap_or(0.0) * 100.0,
                b.0 >> 20,
                b.1 >> 20,
                pace.speed.map_or("-".to_string(), |v| format!("{:.1} MB/s", v / 1048576.0)),
                pace.eta_secs.map_or("-".to_string(), |v| format!("{v:.1} s")),
                fit.text as u8,
                fit.speed as u8,
                fit.eta as u8,
                fit.buttons as u8,
            );
        }
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let ok = worker.join().unwrap();
    println!("finished: ok={ok}, bytes={:?}, history bars (0..1): {:?}", bytes.get(), stats.bars(started.elapsed().as_secs_f64()));
    let _ = std::fs::remove_dir_all(&base);
}
