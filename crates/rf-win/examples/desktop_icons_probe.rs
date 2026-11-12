//! Windowless, read-only probe of the desktop's system icons and of the app's own choice on top of Windows' (Settings -> Masaüstü).
//! Prints: what Windows' own settings say for each of the five icons (shown, and whether by a value or by the default); what each
//! choice (follow / show / hide) makes of it; that the shell has a name for each of the five (its length only); whether the shell can list each as a folder (what
//! decides if a double click opens it in our window); the Recycle Bin as a count and a yes/no; how many bin folders of this user the
//! watch would observe; and an end-to-end run of the item source with the choices applied, over EMPTY scratch folders.
//! No names of the user's files or paths are printed; nothing is written (no registry, no setting, no file) and no window is made.
//!
//!   cargo run --offline -p rf-win --example desktop_icons_probe

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use rf_core::desktop::items::Visibility;
use rf_core::desktop::virtuals::{self, Choices, IconChoice, VirtualKind, Why};
use rf_win::desktop::items::{ChosenVirtuals, DesktopItems, PathFolders, SystemVirtuals, VirtualSource};
use rf_win::desktop::registry::SystemRegistry;

fn main() {
    rf_win::shell::ensure_com();
    let reg = SystemRegistry;
    let windows = virtuals::read(&reg);
    println!("== WINDOWS' OWN SETTING (read-only)");
    for (k, why) in &windows.kinds {
        let how = match why {
            Why::Set(_) => "by a value",
            Why::Default(_) => "by the default",
        };
        println!("  {:<12} shown={} ({how})", k.english_name(), why.shown());
    }
    println!("  show desktop icons: {}   policy NoDesktop: {}", windows.show_icons, windows.policy_no_desktop);

    println!("\n== WHAT EACH CHOICE MAKES OF IT");
    for choice in [IconChoice::Follow, IconChoice::Show, IconChoice::Hide] {
        let mut c = Choices::default();
        for k in VirtualKind::ALL {
            c.set(k, choice);
        }
        let s = virtuals::apply(windows.clone(), &c);
        println!("  all {choice:?}: shown = {:?}", s.shown().iter().map(|k| k.english_name()).collect::<Vec<_>>());
    }
    let master = |m: IconChoice| virtuals::apply(windows.clone(), &Choices { show_icons: m, ..Default::default() }).desktop_disabled();
    println!("  desktop disabled with show-icons follow / show / hide: {} / {} / {}", master(IconChoice::Follow), master(IconChoice::Show), master(IconChoice::Hide));

    println!("\n== THE SHELL'S NAMES, AND WHETHER IT CAN LIST EACH AS A FOLDER");
    for k in VirtualKind::ALL {
        let name = rf_win::shellfolder::display_name(&k.parsing_name());
        let place = rf_win::shellfolder::browsable(&k.parsing_name());
        // (the user's folder is named after the user: only the length of a name is printed)
        println!(
            "  {:<12} shell name has {:?} characters   browsable by us: {}   (it is a folder on disk: {})",
            k.english_name(),
            name.as_ref().map(|n| n.chars().count()),
            place.is_some(),
            place.as_ref().is_some_and(|p| p.fs_path.is_some())
        );
    }
    println!("  the user's folder is found: {}", rf_win::desktop::items::profile_dir().is_some_and(|p| p.is_dir()));

    println!("\n== THE RECYCLE BIN (counts only)");
    match rf_win::recycle::info() {
        Ok((n, _bytes)) => println!("  items: {n}  full: {}", n > 0),
        Err(e) => println!("  not read: {e:#}"),
    }
    println!("  bin folders of this user the watch would observe: {}", rf_win::desktop::binwatch::existing_bin_dirs().len());

    println!("\n== END TO END: the item source with the choices applied, over empty scratch folders");
    let dir = std::env::temp_dir().join(format!("rf-icons-probe-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    // 0 = follow everything, 1 = every icon shown (and the master switch), 2 = every icon hidden (master switch on), 3 = the master
    // switch hidden, 4 = only the master switch shown (Windows may have it off, as it has on some machines)
    let mode = Arc::new(AtomicU8::new(0));
    let m2 = mode.clone();
    let choices: Arc<dyn Fn() -> Choices + Send + Sync> = Arc::new(move || {
        let mut c = Choices::default();
        match m2.load(Ordering::SeqCst) {
            1 => {
                VirtualKind::ALL.iter().for_each(|k| c.set(*k, IconChoice::Show));
                c.show_icons = IconChoice::Show;
            }
            2 => {
                VirtualKind::ALL.iter().for_each(|k| c.set(*k, IconChoice::Hide));
                c.show_icons = IconChoice::Show;
            }
            3 => c.show_icons = IconChoice::Hide,
            4 => c.show_icons = IconChoice::Show,
            _ => {}
        }
        c
    });
    let inner: Arc<dyn VirtualSource> = Arc::new(SystemVirtuals::new(Arc::new(SystemRegistry)));
    let source = Arc::new(ChosenVirtuals { inner, choices });
    let items = DesktopItems::start(Arc::new(PathFolders { user: Some(dir.clone()), public: None }), source, Visibility::default(), false, |_| {});
    items.watch_recycle_bin();
    for (label, m) in [("follow", 0u8), ("show all", 1), ("hide all", 2), ("master switch hidden", 3), ("master switch shown", 4), ("follow again", 0)] {
        mode.store(m, Ordering::SeqCst);
        items.refresh_virtuals();
        let shown = items.items();
        let bin = shown.iter().find(|i| matches!(i.origin, rf_core::desktop::items::Origin::Virtual(VirtualKind::RecycleBin)));
        println!("  {label:<22} items: {}   bin picture state: {:?}", shown.len(), bin.map(|b| b.recycle_full));
    }
    drop(items);
    let _ = std::fs::remove_dir_all(&dir);
    println!("\nDone. Nothing was written.");
}
