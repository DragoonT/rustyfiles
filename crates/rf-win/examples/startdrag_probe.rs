//! Windowless, read-only probe for the Start panel's drag and drop (`rf_win::dock::startdrag`). It shows nothing, sends no input and
//! writes nothing outside a temp folder it makes and removes. It prints COUNTS only (no app names, no paths):
//!
//! * how many apps "All apps" lists, how many are packaged (no shortcut file to drag) and how many classic ones have a shortcut in the
//!   Start menu's folders, found by name;
//! * what the dock would keep for each kind of app (`dock_pin_of`: a package pin; a shortcut pin read on the shell thread; a program
//!   pin) and how long the lookups take;
//! * that the drag copy of a real shortcut is made in a temp folder, is what the original is, and goes away with `sweep`.
//!
//! Run: `cargo run --offline --example startdrag_probe -p rf-win`

use std::time::Instant;

use rf_win::dock::model::{Launch, looks_packaged};
use rf_win::dock::startdrag;

fn main() {
    let t = Instant::now();
    let apps = rf_win::apps::list().unwrap_or_default();
    println!("apps listed: {} ({} ms)", apps.len(), t.elapsed().as_millis());

    let packaged = apps.iter().filter(|a| looks_packaged(&a.id)).count();
    let t = Instant::now();
    let with_shortcut: Vec<_> = apps.iter().filter(|a| startdrag::shortcut_of(a).is_some()).collect();
    println!("packaged (no shortcut file): {packaged}");
    println!("classic with a Start-menu shortcut found by name: {} of {} ({} ms for all lookups, the folders walked once)", with_shortcut.len(), apps.len() - packaged, t.elapsed().as_millis());
    println!("classic without one (dock only): {}", apps.len() - packaged - with_shortcut.len());

    // what the dock keeps for each app: a sample of each kind (the shell thread reads the shortcut)
    let (mut pkg_ok, mut pkg_n, mut lnk_ok, mut lnk_n, mut link_launch, mut exe_launch) = (0, 0, 0, 0, 0, 0);
    let t = Instant::now();
    for a in apps.iter().filter(|a| looks_packaged(&a.id)).take(20) {
        pkg_n += 1;
        if startdrag::dock_pin_of(a, None).is_some_and(|p| matches!(p.launch, Launch::Packaged(_))) {
            pkg_ok += 1;
        }
    }
    for a in with_shortcut.iter().take(20) {
        lnk_n += 1;
        let link = startdrag::shortcut_of(a);
        if let Some(p) = startdrag::dock_pin_of(a, link.as_deref()) {
            lnk_ok += 1;
            match p.launch {
                Launch::Link(_) => link_launch += 1,
                Launch::Exe { .. } => exe_launch += 1,
                Launch::Packaged(_) => {}
            }
        }
    }
    println!("dock pins made: packaged {pkg_ok} of {pkg_n} sampled; classic with shortcut {lnk_ok} of {lnk_n} sampled ({link_launch} start the shortcut, {exe_launch} the program) in {} ms", t.elapsed().as_millis());

    // the drag copy of a real shortcut, in a temp folder of our own
    if let Some(first) = with_shortcut.first().and_then(|a| startdrag::shortcut_of(a)) {
        let root = std::env::temp_dir().join(format!("rf-startdrag-probe-{}", std::process::id()));
        let t = Instant::now();
        match startdrag::drag_copy(&first, &root) {
            Ok(copy) => {
                let same = std::fs::read(&copy).ok() == std::fs::read(&first).ok();
                println!("drag copy made in {} us, same bytes as the original: {same}, original still there: {}", t.elapsed().as_micros(), first.exists());
            }
            Err(e) => println!("drag copy failed: {e}"),
        }
        println!("swept: {}", startdrag::sweep(&root, std::time::Duration::ZERO));
        let _ = std::fs::remove_dir_all(&root);
    } else {
        println!("no shortcut found to copy");
    }
}
