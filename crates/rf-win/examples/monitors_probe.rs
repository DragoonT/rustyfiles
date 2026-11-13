//! Windowless, read-only probe of the dock-on-every-monitor rules on THIS PC's real monitors and windows. No window is made, nothing is
//! moved or sent; it reads the monitor list and the task list and prints what the rules would do.
//!
//!     cargo run --offline -p rf-win --example monitors_probe
//!
//! Prints: the monitors (device name, rectangle, work area, scale, primary), the secondary docks the rules want with the switch on and
//! off, what each dock would hold at its real width (with the plan.rs widths written out below), and how many task windows (a COUNT; no
//! title, name or path is read or printed) each monitor has and how many each dock would show of them with "every app everywhere" and
//! "where the window is". Also how long the monitor list and the per-window monitor question take.

use rf_win::dock::monitors::{self, AppsWhere, Wants};

fn main() {
    let t0 = std::time::Instant::now();
    let displays = monitors::displays();
    let list_us = t0.elapsed().as_micros();
    println!("{} monitor(s), listed in {list_us} us", displays.len());
    for d in &displays {
        println!(
            "  {} {}x{} at ({},{}) work {}x{} at {} dpi{}",
            d.name,
            d.info.rect.width(),
            d.info.rect.height(),
            d.info.rect.left,
            d.info.rect.top,
            d.info.work.width(),
            d.info.work.height(),
            d.info.dpi,
            if d.primary { "  (primary)" } else { "" }
        );
    }

    println!("secondary docks wanted: switch on {}, switch off {}", monitors::wanted(true, &displays).len(), monitors::wanted(false, &displays).len());
    let have: Vec<(String, rf_win::dock::MonitorInfo)> = Vec::new();
    println!("changes from no secondary docks, switch on: {:?}", monitors::reconcile(&have, &displays, true).iter().map(|c| format!("{c:?}")).collect::<Vec<_>>());

    // the widths plan.rs gives, dock units: a secondary left island with the field is 8 + 32 + 8 + 220 + 8, the button 8 + 32 + 8 + 32 + 8;
    // the right island with the bell is 8 + clock (7 x 10 + 8) + 4 + 32 + 8; the middle one is eleven icons here (a made-up but typical dock)
    let middle = rf_win::dock::layout::middle_width(&[44.0; 11], 1);
    for d in displays.iter().filter(|d| !d.primary) {
        let unit = d.info.scale().max(0.5);
        let w = Wants { area: d.info.work.width() as f32 / unit, left_field: 276.0, left_button: 88.0, right_with_bell: 134.0, middle };
        println!("  {} holds {:?} at {:.0} units wide (a made-up middle island of {middle:.0})", d.name, monitors::content(false, &w), w.area);
        let narrow = Wants { area: 640.0, ..w };
        println!("  ... and at 640 units: {:?}", monitors::content(false, &narrow));
    }

    // the windows: counts only
    let t1 = std::time::Instant::now();
    let tasks = rf_win::tasklist::tasks();
    let listed_us = t1.elapsed().as_micros();
    let t2 = std::time::Instant::now();
    let on: Vec<Option<isize>> = tasks.iter().map(|t| monitors::monitor_of_window(t.hwnd)).collect();
    let asked_us = t2.elapsed().as_micros();
    println!("{} task window(s) listed in {listed_us} us; the monitor of each asked in {asked_us} us in all ({} us each)", tasks.len(), asked_us / tasks.len().max(1) as u128);
    println!("  with no monitor told: {}", on.iter().filter(|m| m.is_none()).count());
    let groups = rf_win::tasklist::grouped(tasks);
    for d in &displays {
        let there = on.iter().filter(|m| **m == Some(d.info.handle)).count();
        let apps_all = groups.len();
        let apps_here = monitors::filter_groups(AppsWhere::WindowMonitor, d.info.handle, groups.clone(), |t| monitors::monitor_of_window(t.hwnd)).len();
        println!("  {}: {there} window(s); apps shown: {apps_all} (every app everywhere), {apps_here} (where the window is)", d.name);
    }
}
