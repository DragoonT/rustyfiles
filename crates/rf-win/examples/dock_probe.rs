//! Windowless, read-only probe of the dock's identities: for every pin of the Windows taskbar what its shortcut says (program,
//! AppUserModelID) and what the dock makes of it (alive, moved to a newer install, dead); for every running group what its windows say
//! (their own AppUserModelID, their process's package id) and which pin it is merged into. Nothing is written, no window is created.
//! Prints program names and ids, not window titles.
//!
//!   cargo run -p rf-win --example dock_probe

use std::path::Path;

use rf_win::dock::identity;
use rf_win::dock::model::{self, DockItem};
use rf_win::dock::pins::{self, Fate, RealDisk};

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn main() {
    let reports = pins::inspect_pins(&RealDisk);
    println!("== PINS (the taskbar's order)");
    for r in &reports {
        let (target, aumid, icon) = match &r.facts {
            Some(f) => (name_of(&f.target), f.aumid.clone(), if f.icon_file.is_empty() { "-".into() } else { name_of(Path::new(&f.icon_file)) }),
            None => ("(package, no shortcut)".into(), Some(r.file.clone()), "-".into()),
        };
        let fate = match &r.fate {
            Fate::Alive => "alive".to_string(),
            Fate::Moved(p) => format!("MOVED -> {}", p.display()),
            Fate::Dead(why) => format!("DEAD ({why:?}), left out"),
        };
        println!("  {:<30} program={target:<16} aumid={:<60} icon={icon:<18} {fate}", r.file, aumid.as_deref().unwrap_or("-"));
    }
    let pins_shown: Vec<_> = reports.iter().filter_map(|r| r.app.clone()).collect();

    println!("\n== RUNNING GROUPS (before: the window's own id; after: with the process's package id)");
    let tasks = rf_win::tasklist::tasks();
    for (_, ts) in rf_win::tasklist::grouped(tasks) {
        let t = &ts[0];
        println!(
            "  before: {:<28} window id={:<12} process package id={}",
            t.exe.as_deref().map(name_of).unwrap_or_default(),
            t.app_id.as_deref().unwrap_or("-"),
            identity::process_aumid(t.pid).as_deref().unwrap_or("-")
        );
    }
    let groups = identity::running_groups();
    println!("\n== THE DOCK'S MIDDLE ISLAND (pins merged with the groups)");
    let items = model::merge(&pins_shown, groups, rf_win::dock::foreground_root());
    for it in &items {
        if let DockItem::App(a) = it {
            println!("  {:<30} {:<7} {:<8} windows={} icon={}", a.name, if a.pinned { "pinned" } else { "running" }, if a.running() { "running" } else { "idle" }, a.windows.len(), match &a.icon {
                rf_win::dock::IconSource::Packaged(_) => "package logo".to_string(),
                rf_win::dock::IconSource::File(p) => format!("file {}", name_of(p)),
                rf_win::dock::IconSource::Resource { file, index } => format!("resource {}#{index}", name_of(file)),
                other => format!("{other:?}"),
            });
        }
    }
}
