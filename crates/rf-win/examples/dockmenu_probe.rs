//! Windowless, read-only probe of what the dock's app menu, its own pin list and the hover previews are built from (shell plan increment
//! B8). Prints COUNTS for this machine, never a title, name or path: the apps the dock would show, how many are running, pinned (by the
//! Windows taskbar) and found again in a dry-run seeding of the dock's own list, which fixed rows each app's menu would get, how many
//! windows there are (minimised ones get the large icon in a preview, the others a live picture), the grid the biggest preview would
//! have, and the time the data takes to read.
//!
//! Nothing is written or changed, nothing is shown and no window is created: the taskbar's pins are read (the pin folder and the
//! `Taskband` value, read-only), the window list is a snapshot, the pin file of the dock (if there is one) is read and never written.
//! The seeding is done in memory.
//!
//!   cargo run -p rf-win --example dockmenu_probe

use std::path::PathBuf;
use std::time::Instant;

use rf_win::dock::appmenu::{self, Fixed};
use rf_win::dock::model::DockItem;
use rf_win::dock::ownpins::{self, Loaded, PinFile};
use rf_win::dock::{identity, model, pins, preview, winops};

fn own_pin_file() -> PathBuf {
    let dir = std::env::var_os("RUSTYFILES_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rustyfiles")))
        .unwrap_or_default();
    dir.join(ownpins::FILE)
}

fn main() {
    let t = Instant::now();
    let windows_pins = pins::read_pins();
    println!("== WINDOWS TASKBAR PINS (read only): {} in {:?}", windows_pins.len(), t.elapsed());

    let file = own_pin_file();
    match ownpins::load_checked(&file) {
        Loaded::Absent => println!("== DOCK'S OWN PIN LIST: no file (the island follows the taskbar)"),
        Loaded::Unreadable => println!("== DOCK'S OWN PIN LIST: a file that cannot be read (the island follows the taskbar; it would be set aside, never written over)"),
        Loaded::Found(f) => println!("== DOCK'S OWN PIN LIST: a file, seeded: {}, {} pins", f.seeded, f.pins.len()),
    }

    let t = Instant::now();
    let groups = identity::running_groups();
    let took_groups = t.elapsed();
    let foreground = rf_win::dock::foreground_root();
    let items = model::merge(&windows_pins, groups, foreground);
    let apps: Vec<_> = items.iter().filter_map(|i| if let DockItem::App(a) = i { Some(a) } else { None }).collect();
    let running: Vec<_> = apps.iter().filter(|a| a.running()).collect();
    println!("== DOCK ITEMS: {} apps ({} running, {} pinned by the taskbar), groups read in {:?}", apps.len(), running.len(), apps.iter().filter(|a| a.pinned).count(), took_groups);

    // the dry-run seeding: every taskbar-pinned app must be found again in the dock's own list, and no unpinned app may be
    let mut seeded = PinFile::default();
    seeded.seed(&windows_pins);
    let found = apps.iter().filter(|a| a.pinned && seeded.find(a).is_some()).count();
    let pinned = apps.iter().filter(|a| a.pinned).count();
    let false_hits = apps.iter().filter(|a| !a.pinned && seeded.find(a).is_some()).count();
    println!("== DRY-RUN SEEDING: {} pins copied; {found} of {pinned} pinned apps found again; {false_hits} unpinned apps wrongly found", seeded.pins.len());

    // the fixed rows of every app's menu
    let (mut open, mut pin, mut unpin, mut close_one, mut close_all, mut nothing_to_pin) = (0, 0, 0, 0, 0, 0);
    for a in &apps {
        let can_pin = ownpins::pin_from_app(a).is_some();
        if !can_pin && !a.pinned {
            nothing_to_pin += 1;
        }
        for row in appmenu::fixed_rows(a.launch.is_some(), can_pin, a.pinned, a.windows.len()) {
            match row {
                Fixed::Open => open += 1,
                Fixed::Pin => pin += 1,
                Fixed::Unpin => unpin += 1,
                Fixed::CloseWindow => close_one += 1,
                Fixed::CloseAll(_) => close_all += 1,
            }
        }
    }
    println!("== APP MENUS: rows over all apps: name {open}, pin {pin}, unpin {unpin}, close one window {close_one}, close all {close_all}; apps that cannot be pinned: {nothing_to_pin}");

    // the windows the previews would show
    let t = Instant::now();
    let (mut total, mut minimized, mut untitled) = (0usize, 0usize, 0usize);
    for a in &running {
        for &w in &a.windows {
            total += 1;
            if winops::is_minimized(w) {
                minimized += 1;
            }
            if winops::window_title(w).trim().is_empty() {
                untitled += 1;
            }
        }
    }
    println!("== PREVIEW CARDS: {total} windows of {} running apps ({minimized} minimised: large icon; {untitled} without a title), titles read in {:?}", running.len(), t.elapsed());
    if let Some(biggest) = running.iter().map(|a| a.windows.len()).max() {
        let g = preview::grid(biggest, 2560.0);
        let (w, h) = preview::size(&g);
        println!("   the biggest app has {biggest} windows: {} columns x {} rows, {} shown, {} not shown, window {w:.0} x {h:.0} logical px", g.cols, g.rows, g.shown, g.hidden);
    }
}
