//! Opening a folder from the desktop.
//!
//! In the preview (a main-app process) a folder opens in a rustyfiles window of the same process. In the shell host (the dock's
//! process, a SIDE process: no tray icon, no single-instance forwarding, it never saves the settings) it must not: a file window made
//! there would be a second, half-wired copy of the app inside the thing that draws the user's taskbar and desktop, and it would
//! die with it. There the folder is handed to the main rustyfiles the way a double click on a folder in Explorer is: by starting
//! `rustyfiles.exe <folder>`, which a running rustyfiles answers by opening a tab and which starts one otherwise.

use gpui_kit::App;
use rf_core::Location;

/// The command-line word that makes the main rustyfiles open this place, if the place has one.
pub fn launch_arg(parsing_name: Option<&str>, loc: &Location) -> Option<String> {
    match loc {
        Location::Path(p) => Some(p.to_string_lossy().into_owned()),
        // a shell place (This PC, the Recycle Bin, ...): the name the shell knows it by, which the launcher turns back into the place
        _ => parsing_name.map(str::to_string),
    }
}

/// Whether this process is a side process (the shell host / dock) and so hands folders to the main app.
pub fn hands_over() -> bool {
    crate::background::DOCK_PROCESS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Opens `loc` (`parsing_name`: its shell name, for places that are not folders on a disk) in a rustyfiles window.
pub fn open_location(parsing_name: Option<&str>, loc: Location, cx: &mut App) {
    if !hands_over() {
        crate::open_main_window(Some(loc), None, None, cx);
        return;
    }
    let Some(arg) = launch_arg(parsing_name, &loc) else { return };
    match std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe).arg(&arg).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()
    }) {
        Ok(_) => {}
        Err(e) => log::warn!("desktop: could not hand a folder to rustyfiles: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{hands_over, launch_arg};
    use rf_core::Location;
    use std::path::PathBuf;

    #[test]
    fn a_folder_is_handed_over_by_its_path_and_a_shell_place_by_its_name() {
        assert_eq!(launch_arg(None, &Location::Path(PathBuf::from(r"C:\Users\x\Desktop\Sub"))).as_deref(), Some(r"C:\Users\x\Desktop\Sub"));
        assert_eq!(launch_arg(Some("shell:RecycleBinFolder"), &Location::RecycleBin).as_deref(), Some("shell:RecycleBinFolder"));
        assert_eq!(launch_arg(None, &Location::RecycleBin), None, "a place with no name to hand over is not guessed");
    }

    #[test]
    fn the_preview_opens_its_own_windows() {
        // (the flag is set by the dock's start-up only)
        assert!(!hands_over());
    }
}
