//! Prints the dock's size slider as numbers: the 16 positions with their percent text, unit, island heights and the band the dock reserves
//! (1920 x 1080 monitor at 100 %, 48 px taskbar), how old and new values are read, what a simulated drag asks the dock to do (a second
//! of pointer events, one every millisecond, and one every 4 ms), and how a settings-file read that arrives during a drag is treated.
//! Pure arithmetic and a clock that is a counter: no window, no pipe, no file, nothing is changed.
//! `cargo run --offline -p rf-win --example dock_size_live_probe`

use std::time::{Duration, Instant};

use rf_win::dock::layout::{self, Rect, Widths};
use rf_win::dock::size::unit;
use rf_win::dock::size_live::{self as live, Act, File, Live, Msg, Outbox};

fn main() {
    let work = Rect::new(0, 0, 1920, 1032);
    let widths = Widths { left: 330.0, middle: layout::middle_width(&[44.0; 8], 1), right: 280.0 };
    println!("== the 16 positions (1920 x 1080 at 100 %, work area 1920 x 1032) ==");
    for pos in 0..=live::POSITIONS {
        let pm = live::at_position(pos);
        let u = unit(1.0, live::factor(pm));
        let i = layout::islands(work, u, widths);
        println!(
            "pos {pos:2}  {pm:4}  {:>5} / {:<5}  unit {u:.4}  middle {} x {}  side {} x {}  reserved band {} px",
            live::percent_text(pm, true),
            live::percent_text(pm, false),
            i.middle.width(),
            i.middle.height(),
            i.left.width(),
            i.left.height(),
            layout::reserved_thickness(u)
        );
    }

    println!("\n== reading values ==");
    for name in ["small", "medium", "large", "xlarge", "gigantic"] {
        println!("name {name:>9} -> {:?}", live::from_legacy_name(name));
    }
    for v in [1.0, 1.25, 1.23, 0.75, 1.5, 0.7, 0.0, -3.0, 2.0, 1e300, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        println!("number {v:>10e} -> {}", live::from_number(v));
    }

    println!("\n== a drag: 0.75 -> 1.5 -> 1.0 over one second, an event every {{1, 4}} ms, released at the end ==");
    for every in [1u64, 4] {
        let t0 = Instant::now();
        let mut l = Live::new(live::DEFAULT);
        let (layouts, settles) = (std::cell::Cell::new(0u32), std::cell::Cell::new(0u32));
        let (mut events, mut stamp) = (0u32, 0u64);
        let mut order: Vec<&'static str> = Vec::new();
        let mut due: Option<Instant> = None;
        let mut outbox = Outbox::new();
        let mut written = 0u32;
        let run = |out: live::Out, order: &mut Vec<&'static str>, due: &mut Option<Instant>, now: Instant| {
            for act in out.acts {
                match act {
                    Act::Layout(_) => {
                        layouts.set(layouts.get() + 1);
                        order.push("L");
                    }
                    Act::Settle(_) => {
                        settles.set(settles.get() + 1);
                        order.push("S");
                    }
                }
            }
            if let Some(w) = out.wake {
                let at = now + w;
                *due = Some(due.map_or(at, |d| d.min(at)));
            }
        };
        let mut ms = 0u64;
        while ms < 1000 {
            let now = t0 + Duration::from_millis(ms);
            if due.is_some_and(|d| d <= now) {
                due = None;
                run(l.tick(now), &mut order, &mut due, now);
            }
            // a triangle: down to 750, up to 1500, back to 1000
            let pm = if ms < 333 { 1000 - (250 * ms / 333) } else if ms < 666 { 750 + (750 * (ms - 333) / 333) } else { 1500 - (500 * (ms - 666) / 334) };
            stamp += 1;
            events += 1;
            let m = Msg { stamp, permille: live::clamp(pm as u16), released: false };
            // (the outbox of the settings window: a burst is one write)
            outbox.push(m);
            if ms.is_multiple_of(8) {
                while outbox.pop().is_some() {
                    written += 1;
                }
            }
            run(l.message(now, m), &mut order, &mut due, now);
            ms += every;
        }
        let end = t0 + Duration::from_millis(1000);
        let layouts_while_dragging = layouts.get();        stamp += 1;
        outbox.push(Msg { stamp, permille: 1000, released: true });
        while outbox.pop().is_some() {
            written += 1;
        }
        run(l.message(end, Msg { stamp, permille: 1000, released: true }), &mut order, &mut due, end);
        let settles = settles.get();
        println!(
            "every {every} ms: {events} pointer events -> {layouts_while_dragging} layouts while dragging (one a frame at most: {} in 1000 ms), then at the release {} more, {settles} settle (reserved band + icons), the last acts in order: {:?}, pipe writes ~{written}",            1000 / live::FRAME.as_millis() + 1,
            layouts.get() - layouts_while_dragging,
            &order[order.len().saturating_sub(3)..]
        );
    }

    println!("\n== a drag that is never released: the rest check settles it after {} ms ==", live::IDLE.as_millis());
    let t0 = Instant::now();
    let mut l = Live::new(live::DEFAULT);
    let mut out = l.message(t0, Msg { stamp: 1, permille: 1300, released: false });
    let mut now = t0;
    let mut log = vec![format!("t+0: {:?}", out.acts)];
    while let Some(w) = out.wake {
        now += w;
        out = l.tick(now);
        log.push(format!("t+{}: {:?}", (now - t0).as_millis(), out.acts));
    }
    println!("{}", log.join("   "));

    println!("\n== the settings file read during and after a drag (it may hold an earlier step) ==");
    let mut l = Live::new(live::DEFAULT);
    let t0 = Instant::now();
    l.message(t0, Msg { stamp: 1, permille: 1300, released: true });
    for after in [100u64, 1000, 1499, 1500, 4000] {
        let mut copy = Live::new(live::DEFAULT);
        copy.message(t0, Msg { stamp: 1, permille: 1300, released: true });
        let v = copy.file(t0 + Duration::from_millis(after), 1100);
        println!("file says 1100, {after:4} ms after the release: {}", match v {
            File::Keep { .. } => "kept the slider's 1300".to_string(),
            File::Take { settle } => format!("took the file's 1100 (settle: {settle})"),
        });
    }
    println!("\nout of order: a step stamped 1 after a release stamped 2 -> {:?}", {
        let mut l = Live::new(live::DEFAULT);
        l.message(t0, Msg { stamp: 2, permille: 1300, released: true });
        l.message(t0, Msg { stamp: 1, permille: 900, released: false })
    });
    println!("wire: {:?} -> {:?}", live::message(Msg { stamp: 7, permille: 1250, released: true }), live::parse_message("size 7 1250 1"));
}
