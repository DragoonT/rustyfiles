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
    let (mut open, mut pin, mut unpin, mut close_one, mut close_all, mut end, mut nothing_to_pin) = (0, 0, 0, 0, 0, 0, 0);
    // what "end task" would end for each running app (read here, never done): the processes behind its windows
    let mut end_lines: Vec<String> = Vec::new();
    for a in &apps {
        let can_pin = ownpins::pin_from_app(a).is_some();
        if !can_pin && !a.pinned {
            nothing_to_pin += 1;
        }
        let targets = rf_win::dock::winops::end_task_processes(&rf_win::dock::winops::refs(&a.windows));
        if a.running() {
            end_lines.push(format!("   {:<28} {} window(s) -> {}", a.name.chars().take(28).collect::<String>(), a.windows.len(), if targets.is_empty() { "no row (the shell's, or a frame with nothing inside)".to_string() } else { format!("{} process(es): {targets:?}", targets.len()) }));
        }
        for row in appmenu::fixed_rows(a.launch.is_some(), a.launch.as_ref().is_some_and(|l| l.file().is_some()), can_pin, a.pinned, a.windows.len(), !targets.is_empty()) {
            match row {
                Fixed::Open => open += 1,
                Fixed::Pin => pin += 1,
                Fixed::Unpin => unpin += 1,
                Fixed::CloseWindow => close_one += 1,
                Fixed::CloseAll(_) => close_all += 1,
                Fixed::EndTask => end += 1,
                // (the rows of an app's file are not counted by this probe)
                Fixed::RunAsAdmin | Fixed::OpenLocation | Fixed::Properties | Fixed::MoveToMonitor(_) | Fixed::FitToScreen | Fixed::PinStart | Fixed::UnpinStart => {}
            }
        }
    }
    println!("== APP MENUS: rows over all apps: name {open}, pin {pin}, unpin {unpin}, close one window {close_one}, close all {close_all}, end task {end}; apps that cannot be pinned: {nothing_to_pin}");
    println!("== END TASK would end (nothing is ended by this probe):");
    for l in &end_lines {
        println!("{l}");
    }

    // each running app's own sound level, as the wheel over its icon would find it (read only)
    let all: Vec<isize> = apps.iter().flat_map(|a| a.windows.iter().copied()).collect();
    println!("== APP SOUND (read only; the wheel's lines per notch: {}):", rf_win::appvolume::wheel_lines());
    for a in apps.iter().filter(|a| a.running()) {
        let t = Instant::now();
        let (n, level) = rf_win::appvolume::read(&a.windows, &all);
        let name: String = a.name.chars().take(28).collect();
        match level {
            Some(l) => println!("   {name:<28} {n} session(s), {} %{}  ({:?})", l.percent(), if l.muted { ", muted" } else { "" }, t.elapsed()),
            None => println!("   {name:<28} no sound session  ({:?})", t.elapsed()),
        }
    }

    for (pid, owner, level) in rf_win::appvolume::probe(&all) {
        let exe = |p: u32| rf_win::tasklist::process_exe_path(p).and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".into());
        println!("   session of {pid} ({}) -> {} : {}", exe(pid), owner.map_or("nobody".to_string(), |o| format!("{o} ({})", exe(o))), level.map_or("?".to_string(), |l| format!("{} %", l.percent())));
    }

    // the input language (read only)
    let layouts = rf_win::inputlang::layouts();
    let now = rf_win::inputlang::current();
    println!("== INPUT LANGUAGE: {} layout(s): {:?}; the window in front types in {:?}", layouts.len(), layouts.iter().map(|l| rf_win::inputlang::code(*l)).collect::<Vec<_>>(), now.code);

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
        let (w, h) = preview::layout(&vec![None; biggest], 2560.0).size();
        println!("   the biggest app has {biggest} windows: {} columns x {} rows, {} shown, {} not shown, window {w:.0} x {h:.0} logical px (shapes unknown: 16:10)", g.cols, g.rows, g.shown, g.hidden);
    }
    // the dynamic height: every running window's box from its own rectangle (a read, no DWM registration, nothing drawn); counts only
    {
        use rf_win::switcher::thumbs::{Dwm, ThumbApi, usable_size};
        let (mut low, mut mid, mut high, mut unknown) = (0usize, 0usize, 0usize, 0usize);
        for a in &running {
            for &w in &a.windows {
                match usable_size(Dwm.window_size(w), winops::is_minimized(w)) {
                    None => unknown += 1,
                    Some(s) => {
                        let ph = preview::pic_height(Some(s));
                        if ph <= (preview::PIC_W * preview::MIN_PIC_RATIO).round() {
                            low += 1
                        } else if ph >= (preview::PIC_W * preview::MAX_PIC_RATIO).round() {
                            high += 1
                        } else {
                            mid += 1
                        }
                    }
                }
            }
        }
        println!("   box heights of those windows: {low} at the lowest limit, {mid} between, {high} at the highest limit, {unknown} not known (16:10)");
    }
}
