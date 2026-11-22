//! Settings > Appearance > "App icon": the icon of rustyfiles' own shortcuts (the Start menu's, the desktop's, the one pinned to the
//! taskbar), which an update put back to the default every time someone changed it by hand (a Discord request, 2026-11-22). The chosen
//! icon is written into those shortcuts when it is chosen, after an install or an update, and at every start (an update may write the
//! shortcuts again after its own hook has run); an empty icon gives the shortcut back the program's own.
//!
//! Only shortcuts whose target is a `rustyfiles.exe` are touched: a user's own shortcut of the same name to something else stays.

use std::path::{Path, PathBuf};

use crate::shortcut;

/// The name Velopack gives the shortcuts (the package title).
const LINK: &str = "rustyfiles.lnk";

/// The places rustyfiles' shortcuts are kept in: the Start menu's programs, the desktop, the taskbar's pins.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        out.push(appdata.join(r"Microsoft\Windows\Start Menu\Programs").join(LINK));
        out.push(appdata.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar").join(LINK));
    }
    if let Some(desktop) = crate::desktop::items::user_desktop_dir() {
        out.push(desktop.join(LINK));
    }
    out
}

/// Whether a shortcut's target is rustyfiles' program.
fn points_at_us(target: &str) -> bool {
    Path::new(target).file_name().is_some_and(|n| n.eq_ignore_ascii_case("rustyfiles.exe"))
}

/// The icon (`path`, index) a shortcut should have for the choice `wanted` (`None`: the program's own, an empty location).
fn wanted_location(wanted: Option<&str>) -> (String, i32) {
    match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => (w.to_string(), 0),
        None => (String::new(), 0),
    }
}

/// Writes the icon `wanted` into each of rustyfiles' shortcuts in `links` that does not have it yet; returns how many were changed.
/// Errors on one shortcut (a read-only file, a pin that is gone) do not stop the others. The shell is not told ([`apply`] tells it).
pub fn apply_to(links: &[PathBuf], wanted: Option<&str>) -> usize {
    let (icon, index) = wanted_location(wanted);
    let mut changed = 0;
    for link in links.iter().filter(|l| l.is_file()) {
        let Ok(mut d) = shortcut::details(link) else { continue };
        if !points_at_us(&d.target) || (d.icon.eq_ignore_ascii_case(&icon) && d.icon_index == index) {
            continue;
        }
        d.icon = icon.clone();
        d.icon_index = index;
        match shortcut::save(link, &d) {
            Ok(()) => changed += 1,
            Err(e) => log::warn!("app icon: {}: {e:#}", link.display()),
        }
    }
    changed
}

/// [`apply_to`] the usual places ([`candidates`]), and the shell is told when anything changed (it keeps icons in its cache).
pub fn apply(wanted: Option<&str>) -> usize {
    let changed = apply_to(&candidates(), wanted);
    if changed > 0 {
        unsafe {
            use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_own_shortcuts_get_the_icon_and_the_default_clears_it() {
        crate::shell::ensure_com();
        let dir = std::env::temp_dir().join(format!("rf-app-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rustyfiles.exe");
        let other = dir.join("notepad.exe");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let ours = dir.join("ours.lnk");
        let theirs = dir.join("theirs.lnk");
        shortcut::create(&ours, &exe).unwrap();
        shortcut::create(&theirs, &other).unwrap();
        let icon = dir.join("mine.ico").to_string_lossy().into_owned();
        let links = [ours.clone(), theirs.clone(), dir.join("gone.lnk")];
        assert_eq!(apply_to(&links, Some(&icon)), 1, "only the shortcut to rustyfiles.exe");
        assert_eq!(shortcut::details(&ours).unwrap().icon, icon);
        assert_eq!(shortcut::details(&theirs).unwrap().icon, "");
        assert_eq!(apply_to(&links, Some(&icon)), 0, "nothing to do the second time");
        assert_eq!(apply_to(&links, None), 1);
        assert_eq!(shortcut::details(&ours).unwrap().icon, "", "the default: the program's own icon");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
