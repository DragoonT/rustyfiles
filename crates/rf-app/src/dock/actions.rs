//! What the dock's clicks start. Each runs on a thread of its own: starting a program or asking the shell can take a moment and
//! the dock must not stall while it does.
//!
//! Opening rustyfiles things goes through the program itself (`rustyfiles.exe <argument>`): the running rustyfiles takes the
//! argument over its single-instance pipe (a new tab, the quick search), and when none runs this starts one, exactly as a shortcut
//! or the hotkey would.

use rf_win::dock::{ClickAction, DockApp, Launch, Place};

/// The command line that opens the quick search in rustyfiles.
pub fn quick_search_args() -> Vec<String> {
    vec![crate::quick_search::LAUNCH_ARG.to_string()]
}

/// The command line that opens a place in rustyfiles: the argument rustyfiles takes on its own command line.
pub fn place_args(place: Place) -> Vec<String> {
    vec![place.open_argument()]
}

pub(super) fn run_rustyfiles(args: Vec<String>) {
    std::thread::spawn(move || {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                log::warn!("dock: cannot find rustyfiles.exe: {e}");
                return;
            }
        };
        if let Err(e) = std::process::Command::new(exe).args(&args).spawn() {
            log::warn!("dock: cannot start rustyfiles {args:?}: {e}");
        }
    });
}

pub fn open_quick_search() {
    run_rustyfiles(quick_search_args());
}

/// The command line that starts the main rustyfiles without a window (for the file index the dock's search asks it for).
pub fn background_main_args() -> Vec<String> {
    vec![crate::search_pipe::START_ARG.to_string()]
}

/// The user chose "start rustyfiles in the background" in the search results.
pub fn start_main_in_background() {
    run_rustyfiles(background_main_args());
}

/// The command line that opens rustyfiles' settings (the same one the main app's own tray menu leads to).
pub fn settings_args() -> Vec<String> {
    vec!["--settings=general".to_string()]
}

/// The command line that opens a folder with `path` selected in it: Explorer's own `/select,` form, which rustyfiles takes.
pub fn show_in_folder_args(path: &std::path::Path) -> Vec<String> {
    vec![format!("/select,{}", path.display())]
}

pub fn open_place(place: Place) {
    run_rustyfiles(place_args(place));
}

/// A click on an app: start, bring forward or minimise, as `rf_win::dock::model::click_action` decides.
pub fn click_app(app: &DockApp, foreground: isize) {
    perform(rf_win::dock::model::click_action(&app.windows, foreground), app.launch.clone());
}

/// A middle click on an app: another instance.
pub fn middle_click_app(app: &DockApp) {
    perform(rf_win::dock::model::middle_click_action(), app.launch.clone());
}

fn perform(action: ClickAction, launch: Option<Launch>) {
    // Every action talks to another program: starting it, or sending a window of it a show / minimise request. A program that
    // hangs would hold up whichever thread asks, so none of it runs on the UI thread. Any thread of this process may bring a window
    // forward: what lets it is that the process has just received the user's click, not which thread handles it.
    std::thread::spawn(move || {
        if let Err(e) = rf_win::dock::launch::perform(action, launch.as_ref()) {
            log::warn!("dock: {action:?} failed: {e:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_field_opens_the_quick_search_the_way_the_launch_argument_does() {
        assert_eq!(quick_search_args(), vec!["--quick-search".to_string()]);
    }

    #[test]
    fn the_start_panels_actions_use_the_command_lines_rustyfiles_understands() {
        assert_eq!(background_main_args(), vec!["--no-window".to_string()]);
        assert_eq!(settings_args(), vec!["--settings=general".to_string()]);
        assert_eq!(show_in_folder_args(std::path::Path::new(r"C:\Users\x\a b\c.txt")), vec![r"/select,C:\Users\x\a b\c.txt".to_string()]);
    }

    #[test]
    fn the_places_open_with_their_own_argument() {
        let bin = place_args(Place::RecycleBin);
        assert_eq!(bin, vec!["shell:RecycleBinFolder".to_string()]);
        let downloads = place_args(Place::Downloads);
        assert_eq!(downloads.len(), 1);
        assert!(!downloads[0].is_empty());
    }
}
