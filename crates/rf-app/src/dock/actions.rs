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

pub(crate) fn run_rustyfiles(args: Vec<String>) {
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

/// Whether a start of Task Manager is under way or has just been made (a second right click in that moment must not start another).
static TASK_MANAGER: std::sync::Mutex<rf_win::dock::taskmgr::StartGate> = std::sync::Mutex::new(rf_win::dock::taskmgr::StartGate::new());

/// The clock the gate is timed on: milliseconds since the first look at it.
fn gate_ms() -> u64 {
    use std::sync::LazyLock;
    static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    START.elapsed().as_millis() as u64
}

/// A right click on the empty glass of an island: Windows' Task Manager, as a right click on Windows' taskbar offers it. Started
/// through the shell's "open" (no elevation is asked for by us: Task Manager's own manifest decides what it needs), once: a second
/// right click while it starts, or in the moment after, starts nothing. If Task Manager is already open, starting it again brings it
/// forward. The shell call is made on the shell thread (`rf_win::sta`), not on a thread of our own: a worker that calls the shell
/// becomes an apartment that never pumps, which that module's comment says the shell does not survive. The gate is opened from the
/// same job when the call returns, so it stays shut for as long as the call is held (an elevation prompt waiting for an answer).
pub fn open_task_manager() {
    let go = TASK_MANAGER.lock().map(|mut g| g.begin(gate_ms())).unwrap_or(false);
    if !go {
        return;
    }
    rf_win::sta::run(|_| {
        match rf_win::dock::launch::start(&rf_win::dock::taskmgr::launch()) {
            Ok(()) => {}
            // the user said no at the prompt Task Manager's manifest asked for: an answer, not a fault
            Err(e) if rf_win::dock::taskmgr::was_cancelled(&e) => log::debug!("dock: Task Manager was not started: the prompt was declined"),
            Err(e) => log::warn!("dock: cannot start Task Manager: {e:#}"),
        }
        if let Ok(mut g) = TASK_MANAGER.lock() {
            g.finished(gate_ms());
        }
    });
}

/// A click on an app: start, bring forward or minimise, as `rf_win::dock::model::click_action` decides.
pub fn click_app(app: &DockApp, foreground: isize) {
    perform(rf_win::dock::model::click_action(&app.windows, foreground), app.launch.clone(), |_| {});
}

/// [`click_app`] that says how it went: `done` is called from the worker thread when the action is over, with `None` when it worked and
/// with the sentence for the user (`shellless::start_failure_text`, the one the Start panel shows) when it failed. The dock's icon shakes
/// on it.
pub fn click_app_reporting(app: &DockApp, foreground: isize, done: impl FnOnce(Option<String>) + Send + 'static) {
    perform(rf_win::dock::model::click_action(&app.windows, foreground), app.launch.clone(), done);
}

/// A middle click on an app: another instance.
pub fn middle_click_app(app: &DockApp) {
    perform(rf_win::dock::model::middle_click_action(), app.launch.clone(), |_| {});
}

fn perform(action: ClickAction, launch: Option<Launch>, done: impl FnOnce(Option<String>) + Send + 'static) {
    // Every action talks to another program: starting it, or sending a window of it a show / minimise request. A program that
    // hangs would hold up whichever thread asks, so none of it runs on the UI thread. Any thread of this process may bring a window
    // forward: what lets it is that the process has just received the user's click, not which thread handles it.
    std::thread::spawn(move || {
        match rf_win::dock::launch::perform(action, launch.as_ref()) {
            Ok(()) => done(None),
            Err(e) => {
                log::warn!("dock: {action:?} failed: {e:#}");
                done(Some(crate::shellless::start_failure_text(&e)));
            }
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
    fn task_manager_is_started_on_the_shell_thread_and_not_on_a_thread_of_our_own() {
        let src = include_str!("actions.rs");
        let body = src.split("pub fn open_task_manager()").nth(1).and_then(|rest| rest.split("\n}").next()).expect("the function is there");
        assert!(body.contains("rf_win::sta::run("), "the shell call belongs on the shell thread");
        assert!(!body.contains("thread::spawn"), "a worker that calls the shell becomes an apartment that never pumps");
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
