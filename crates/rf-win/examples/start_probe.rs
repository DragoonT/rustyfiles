//! Prints what the Start panel's system sources give on this PC. Read-only and windowless: no window is created, no key is sent,
//! nothing is started, no setting is changed and no power call is made. Names of apps and of the user are not printed, only counts,
//! lengths and times.
//!
//!   cargo run -p rf-win --example start_probe --offline

use std::time::{Duration, Instant};

use rf_win::apps::App;
use rf_win::dock::{power, startapps, startlist, startpins};

fn main() {
    let turkish = rf_win::encoding::user_locale_is_turkish();
    println!("user locale is Turkish: {turkish}");

    // the app list, on the shell thread like the dock does it
    let t = Instant::now();
    let listed = rf_win::sta::call(Duration::from_secs(20), rf_win::apps::list);
    let mut apps: Vec<App> = match listed {
        Some(Ok(a)) => a,
        Some(Err(e)) => {
            println!("apps::list failed: {e:#}");
            return;
        }
        None => {
            println!("apps::list: no answer from the shell thread in 20 s");
            return;
        }
    };
    println!("installed apps: {} (cold list on the shell thread: {} ms)", apps.len(), t.elapsed().as_millis());
    let t = Instant::now();
    let again = rf_win::sta::call(Duration::from_secs(20), rf_win::apps::list);
    println!("  the same list again: {} apps, {} ms", again.and_then(|r| r.ok()).map_or(0, |a| a.len()), t.elapsed().as_millis());

    // the order and the headers
    let t = Instant::now();
    startlist::sort_apps(&mut apps, turkish);
    let sort_us = t.elapsed().as_micros();
    let t = Instant::now();
    let rows = startlist::rows(&apps, turkish);
    let rows_us = t.elapsed().as_micros();
    let headers: Vec<String> = rows.iter().filter_map(|r| if let startlist::Row::Header(h) = r { Some(h.clone()) } else { None }).collect();
    println!("sort: {sort_us} us, rows: {} ({} headers: {}) in {rows_us} us", rows.len(), headers.len(), headers.join(" "));
    let sorted = apps.windows(2).all(|w| startlist::compare(&w[0].name, &w[1].name, turkish) != std::cmp::Ordering::Greater);
    println!("list is in order: {sorted}");

    // the taskbar's pins and what seeding the Start panel from them gives
    let t = Instant::now();
    let taskbar = rf_win::dock::pins::read_pins();
    let seeded = startpins::seed(&taskbar, &apps);
    println!("taskbar pins: {}, seed for Start: {} ({} ms)", taskbar.len(), seeded.len(), t.elapsed().as_millis());
    for tb in &taskbar {
        let hit = seeded.iter().any(|p| apps.iter().any(|a| a.id == p.id && (a.name.to_lowercase() == tb.name.to_lowercase() || tb.aumid.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&a.id)))));
        if !hit {
            println!("  a taskbar pin matched by program or not at all: name length {}", tb.name.chars().count());
        }
    }

    // where a few apps live (the "Dosya konumunu aç" item)
    let t = Instant::now();
    let sample: Vec<&App> = apps.iter().take(30).collect();
    let found = sample.iter().filter(|a| startapps::location(a).is_some()).count();
    println!("program found for {found} of the first {} apps ({} ms for all)", sample.len(), t.elapsed().as_millis());

    // the name of the user, length only
    let n = power::display_name();
    println!("display name: {} characters", n.chars().count());
}
