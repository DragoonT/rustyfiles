//! The middle island's pins, app side. The model, the file and the seeding are `rf_win::dock::ownpins`; this is the path, the reading the
//! island does and the pin / unpin the app menu asks for.
//!
//! WHICH LIST THE ISLAND SHOWS. Until the user pins or unpins from the dock's own menu, the apps pinned to the Windows taskbar (read
//! only, as always). The first such change copies the taskbar's pins into `dock-pins.json` and applies the change; from then on that
//! file is the list and the taskbar's pins are not read for the island. Windows' stores are never written, by this or by anything.

use std::path::PathBuf;

use gpui_kit::*;
use rf_win::dock::ownpins::{self, Changed};
use rf_win::dock::pinorder::Placed;
use rf_win::dock::{DockApp, PinnedApp};

use super::middle::MiddleIsland;

/// The pin file.
pub fn path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join(ownpins::FILE))
}

/// The pins the island shows: the dock's own list when it is in force, else the Windows taskbar's. Blocks (disk, the shell): a worker
/// thread.
pub fn effective_pins() -> Vec<PinnedApp> {
    if let Some(own) = path().and_then(|p| ownpins::effective(&p, |f| f.exists())) {
        return own;
    }
    rf_win::dock::pins::read_pins()
}

/// The island, so that a change of the list shows at once instead of at the island's next look (every 20 s).
struct Island(WeakEntity<MiddleIsland>);

impl Global for Island {}

/// The middle island tells where it is (called once, when it is made).
pub fn register(island: WeakEntity<MiddleIsland>, cx: &mut App) {
    cx.set_global(Island(island));
}

/// Pins (`pin`) or unpins an app on the dock's own list, on a worker thread; the island reads the list again when it is written.
pub fn set_pinned(app: DockApp, pin: bool, cx: &mut App) {
    cx.spawn(async move |cx| {
        let done = cx.background_executor().spawn(async move { apply(&app, pin) }).await;
        match done {
            Ok(Changed::Written) => {
                log::info!("dock: {} {} the dock's own pin list", if pin { "pinned" } else { "took off" }, if pin { "to" } else { "from" });
                cx.update(|cx| {
                    if let Some(island) = cx.try_global::<Island>().map(|i| i.0.clone()) {
                        let _ = island.update(cx, |me, cx| me.reload_pins(cx));
                    }
                });
            }
            Ok(Changed::Same) => log::info!("dock: the pin list needed no change ({} for the app of the menu)", if pin { "it is already on it, or cannot be pinned" } else { "it is not on the dock's own list" }),
            Err(e) => log::warn!("dock: cannot change the pin list: {e:?}"),
        }
    })
    .detach();
}

/// Writes the order the island shows its pinned apps in after an icon was dragged to a new place (`placed`: the pinned apps left to
/// right, by their position among the pins the island showed - `before` are those pins' names, which the file checks its list against -
/// and a running app that was dropped among them: it is pinned there), on a worker thread. The island reads the list again when this
/// is over, whatever came of it: after a write to show what is on disk, after a failure to put the icons back as they are there (the
/// island had already shown the new order, so that the drop does not flicker).
pub fn set_order(placed: Vec<Placed>, before: Vec<String>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let done = cx.background_executor().spawn(async move { apply_with(|f| f.apply_order(&placed, &before, |p| p.exists())) }).await;
        match done {
            Ok(Changed::Written) => log::info!("dock: wrote the dragged order to the dock's own pin list"),
            Ok(Changed::Same) => log::info!("dock: the dragged order needed no change in the pin list"),
            Err(e) => log::warn!("dock: cannot write the dragged order to the pin list: {e:?}"),
        }
        cx.update(|cx| {
            if let Some(island) = cx.try_global::<Island>().map(|i| i.0.clone()) {
                let _ = island.update(cx, |me, cx| me.reload_pins(cx));
            }
        });
    })
    .detach();
}

/// An app of the Start panel was dropped on the dock: it is added to the END of the dock's own list (a drop at a chosen place among the
/// icons is not built), unless it is on it already, which is not said. On a worker thread; the island reads the list when it is written.
pub fn pin_start_app(app: rf_win::apps::App, cx: &mut App) {
    cx.spawn(async move |cx| {
        let done = cx.background_executor().spawn(async move { apply_start_app(&app) }).await;
        match done {
            Ok(Changed::Written) => {
                log::info!("dock: an app of the Start panel was added to the dock's own pin list");
                cx.update(|cx| {
                    if let Some(island) = cx.try_global::<Island>().map(|i| i.0.clone()) {
                        let _ = island.update(cx, |me, cx| me.reload_pins(cx));
                    }
                });
            }
            Ok(Changed::Same) => log::info!("dock: the app dropped on the dock is on the pin list already, or cannot be started again"),
            Err(e) => log::warn!("dock: cannot add the dropped app to the pin list: {e:?}"),
        }
    })
    .detach();
}

fn apply_start_app(app: &rf_win::apps::App) -> Result<Changed, ownpins::ChangeError> {
    use rf_win::dock::startdrag;
    let Some(path) = path() else { return Err(ownpins::ChangeError::Disk("no config folder".into())) };
    // (the shortcut and the shell are asked for before the file is touched; an app that cannot be pinned changes nothing)
    let shortcut = startdrag::shortcut_of(app);
    let Some(pin) = startdrag::dock_pin_of(app, shortcut.as_deref()) else { return Ok(Changed::Same) };
    let folder = path.parent().map(|d| d.join(COPIES));
    let copy_links = {
        let folder = folder.clone();
        move |pins: Vec<PinnedApp>| match folder {
            Some(dir) => ownpins::localize_links(pins, |f| copy_shortcut(f, &dir)),
            None => pins,
        }
    };
    // (the app's own shortcut is copied into the dock's folder only once it is known to be added, and never over a file that is there)
    ownpins::change(&path, read_taskbar_for_seed, copy_links, |f| {
        startdrag::add_to_dock(f, &pin, |p| match &folder {
            Some(dir) => startdrag::localized(p, dir),
            None => p,
        })
    })
}

fn apply(app: &DockApp, pin: bool) -> Result<Changed, ownpins::ChangeError> {
    apply_with(|f| if pin { f.pin(app) } else { f.unpin(app) })
}

fn apply_with(edit: impl FnOnce(&mut ownpins::PinFile) -> bool) -> Result<Changed, ownpins::ChangeError> {
    let Some(path) = path() else { return Err(ownpins::ChangeError::Disk("no config folder".into())) };
    // (the taskbar's pins are asked for only when the list has not been seeded yet; their shortcuts are copied into the dock's own
    // folder only after the change is known to be made, so that the list does not depend on Windows' pin folder any more and a change
    // that changes nothing leaves nothing behind)
    let folder = path.parent().map(|d| d.join(COPIES));
    let copy_links = move |pins: Vec<PinnedApp>| match folder {
        Some(dir) => ownpins::localize_links(pins, |f| copy_shortcut(f, &dir)),
        None => pins,
    };
    ownpins::change(&path, read_taskbar_for_seed, copy_links, edit)
}

/// The taskbar's pins for the seeding, `None` when the read cannot be believed: the shell did not answer, or it answered "no pins" while
/// the pin folder could not be listed or held shortcuts that could not be read (an empty answer like that would make an empty list the
/// dock's list for good).
fn read_taskbar_for_seed() -> Option<Vec<PinnedApp>> {
    use rf_win::dock::pins;
    let reports = rf_win::sta::call(std::time::Duration::from_secs(15), || pins::inspect_pins(&pins::RealDisk))?;
    let apps: Vec<PinnedApp> = reports.iter().filter_map(|r| r.app.clone()).collect();
    if !apps.is_empty() {
        return Some(apps);
    }
    let listed = match pins::pin_folder() {
        Some(dir) => match std::fs::read_dir(&dir) {
            Ok(_) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        },
        None => false,
    };
    let unreadable = reports.iter().filter(|r| r.facts.is_none()).count();
    if ownpins::empty_read_is_believable(listed, unreadable) {
        Some(apps)
    } else {
        log::warn!("dock: the taskbar's pins came back empty but could not be read properly (folder listed: {listed}, unreadable shortcuts: {unreadable}); nothing is seeded");
        None
    }
}

/// The folder next to the pin file that holds the copies of the shortcuts of the taskbar's pins.
const COPIES: &str = "dock-pins";

/// Copies one of Windows' pin shortcuts (read only) into the dock's own folder. `None` when it cannot be copied.
fn copy_shortcut(from: &std::path::Path, dir: &std::path::Path) -> Option<PathBuf> {
    let to = dir.join(from.file_name()?);
    std::fs::create_dir_all(dir).ok()?;
    std::fs::copy(from, &to).ok()?;
    Some(to)
}

#[cfg(test)]
mod tests {
    use super::copy_shortcut;

    #[test]
    fn a_shortcut_is_copied_beside_the_pin_file_and_the_original_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("rf-dockpins-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("Original.lnk");
        std::fs::write(&original, b"link bytes").unwrap();
        let copies = dir.join("dock-pins");
        let copy = copy_shortcut(&original, &copies).unwrap();
        assert_eq!(copy, copies.join("Original.lnk"));
        assert_eq!(std::fs::read(&copy).unwrap(), b"link bytes");
        assert_eq!(std::fs::read(&original).unwrap(), b"link bytes", "the original is untouched");
        assert!(copy_shortcut(&dir.join("missing.lnk"), &copies).is_none(), "a shortcut that is not there gives no copy");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
