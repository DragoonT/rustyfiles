//! Read-only probe of the two fixes of the dock's app menu (shell plan, tray feed and app menu track). Prints COUNTS only: never an
//! app's name, an item's title or a path.
//!
//! 1. THE ICONS OF JUMP-LIST ENTRIES. For every running app (taken as if nothing were pinned), the jump list is read the way the menu
//!    reads it and the entries are counted: how many there are, how many have no icon of their own (the menu draws the app's), and
//!    how many still name an icon inside a `WindowsApps` folder (an execution alias or a package file: must be 0, those were the
//!    generic pictures).
//! 2. THE NAME LINE. For every running app the dock names by its program's file name, the shell is asked what the Start panel and the
//!    file's description call it; the probe counts the apps whose menu name differs from the file name, by source.
//!
//! IT ASKS THE SAME QUESTIONS AS THE MENU, by the same code: an app's AppUserModelID and program come from
//! `model::jump_identity` (what `JumpApp::of` uses), the shell is asked with `launch::packaged_id` + `launch::known_names` (what
//! `jump_source::display_name` asks), and the name is `appmenu::better_name` (what it returns). So the counts are the menu's.
//!
//! Read-only: jump-list files are read into memory, the shell is asked through its own thread (`rf_win::sta`), nothing is started,
//! written, pinned or closed. NO WINDOW IS SHOWN, but the probe does start `rf_win::sta`'s shell thread, which makes one hidden
//! message window (the jump-list reader and the name lookups run there, as in the dock); it is never visible and ends with the process.
//! The tray feed has no probe: it needs a running tray host, which only a human starts (it is tested through fakes of the host's model).
//!
//!   cargo run -p rf-win --example menufix_probe

use std::time::Instant;

use rf_win::dock::model::{self, DockItem, IconSource};
use rf_win::dock::{appmenu, identity, launch};
use rf_win::jumplists::lnk::is_package_path;
use rf_win::jumplists::{self, AppKey, JumpItem};

fn package_icon(i: &JumpItem) -> bool {
    match &i.icon {
        Some(IconSource::File(p)) | Some(IconSource::Resource { file: p, .. }) => is_package_path(p),
        _ => false,
    }
}

fn main() {
    let t = Instant::now();
    let groups = identity::running_groups();
    let items = model::merge(&[], groups, rf_win::dock::foreground_root());
    let apps: Vec<_> = items.iter().filter_map(|i| if let DockItem::App(a) = i { Some(a) } else { None }).collect();
    println!("== RUNNING APPS: {} read in {:?}", apps.len(), t.elapsed());

    // 1. jump-list icons
    let (mut with_list, mut entries, mut no_icon, mut package_icons) = (0usize, 0usize, 0usize, 0usize);
    let t = Instant::now();
    for a in &apps {
        let (aumid, exe) = model::jump_identity(a);
        if aumid.is_none() && exe.is_none() {
            continue;
        }
        let list = jumplists::read(&AppKey { aumid, exe }, 10);
        let all: Vec<&JumpItem> = list.tasks.iter().chain(list.custom.iter().flat_map(|c| &c.items)).chain(&list.pinned).chain(list.automatic()).collect();
        let real: Vec<&&JumpItem> = all.iter().filter(|i| !matches!(i.target, jumplists::JumpTarget::Separator)).collect();
        if !real.is_empty() {
            with_list += 1;
        }
        entries += real.len();
        no_icon += real.iter().filter(|i| i.icon.is_none()).count();
        package_icons += real.iter().filter(|i| package_icon(i)).count();
    }
    println!("== JUMP-LIST ICONS: {with_list} apps with a list, {entries} entries, {no_icon} without an icon of their own (the app's is drawn), {package_icons} still naming an alias or package file (must be 0), read in {:?}", t.elapsed());

    // 2. names
    let t = Instant::now();
    let (mut asked, mut by_display, mut by_description, mut unchanged) = (0usize, 0usize, 0usize, 0usize);
    for a in &apps {
        let (aumid, exe) = model::jump_identity(a);
        asked += 1;
        let known = launch::known_names(launch::packaged_id(aumid.as_deref()), exe.as_deref());
        match appmenu::better_name(a.pinned, &a.name, known.display.as_deref(), known.description.as_deref()) {
            None => unchanged += 1,
            Some(name) if known.display.as_deref().map(str::trim) == Some(name.as_str()) => by_display += 1,
            Some(_) => by_description += 1,
        }
    }
    println!("== NAME LINE: {asked} apps asked; {by_display} get the name \"All apps\" shows, {by_description} the file's description, {unchanged} keep the file name; in {:?}", t.elapsed());
}
