//! Hiding Explorer's taskbar while the dock runs (shell plan E1), the host's side of the setting `shell_hide_taskbar` (off by default;
//! one switch on the dock's Settings page).
//!
//! The work is in `rf_win::taskbar_hide` (a thread of its own, so that nothing waits for Explorer on the interface thread) and, under
//! it, `rf_shellguard` (the rules, tested against a pretend desktop; the guard program). This file only follows the setting:
//!
//! * the setting on (at the host's start, or when a changed settings file is read): the thread starts, the guard is started and
//!   waited for, the record is written, Explorer's taskbar windows are hidden;
//! * the setting off: they are shown again (on a thread, so that the interface never waits), the thread ends;
//! * the host's shutdown: [`stop`] shows them again and waits a few seconds at most; whatever is left is the guard's;
//! * the setting off at the host's start: once per process, if the state file still holds a record of hidden windows (a host that
//!   crashed with the taskbar hidden, and its guard gone too), a one-shot worker undoes it ([`Action::StaleCheck`]). Nothing is
//!   started for a state file without one.
//!
//! WHAT IS CHANGED, and what is not: the visibility of Explorer's taskbar windows, and - while the hide is in force and confirmed - the
//! session's work area, because a hidden taskbar keeps its strip: the dock takes the bottom edge (the work area becomes the monitor less
//! the dock's band, or the whole monitor with `dock_reserve` off; the dock does not register its own band with Explorer meanwhile).
//! The work area is the running session's, never stored; the user's own rectangles are written down first and put back on every way
//! out. NOT the auto-hide state or any other taskbar setting, not the registry. No environment variable: the switch is the switch.
//! docs/design/taskbar-hide.md.
//!
//! The band's thickness comes from the dock (`dock::host::apply_reserve` calls [`band_changed`] with what it reserves, or `None`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::*;
use rf_win::taskbar_hide::{self, Config, TaskbarHider};

use crate::dock::parts::{self, Part};
use crate::settings::Settings;

/// The dock's band as the dock last said it (`None` inside: it reserves none). `None` outside: the dock has not said yet.
static BAND: Mutex<Option<Option<i32>>> = Mutex::new(None);

/// Remembers the dock's band; true when it is not what was remembered before.
fn note_band(band: Option<i32>) -> bool {
    let mut g = BAND.lock().unwrap_or_else(|p| p.into_inner());
    let changed = *g != Some(band);
    *g = Some(band);
    changed
}

/// The band to start the hider with: what the dock said (`Some`, and inside it `None` for "reserves none"), or `None` when it has not
/// said anything yet.
fn current_band() -> Option<Option<i32>> {
    *BAND.lock().unwrap_or_else(|p| p.into_inner())
}

/// The dock reserves `band` px at the bottom of the primary monitor (device px), or none (`None`: `dock_reserve` is off). Costs a lock
/// and a comparison when nothing changed and nothing is running; with the hide in force the work area follows.
pub fn band_changed(band: Option<i32>, cx: &App) {
    if note_band(band)
        && let Some(h) = cx.try_global::<Hiding>()
    {
        h.hider.set_band(band);
    }
}

struct Hiding {
    hider: TaskbarHider,
}

impl Global for Hiding {}

/// Has this process already looked for a record of an earlier host (or started the worker that does, which does it first)?
static STALE_LOOKED: AtomicBool = AtomicBool::new(false);

/// What `on_settings` does about the setting, decided without any window or thread so that every case is a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start the worker (it undoes an earlier host's record first) and hide.
    Start,
    /// Show the taskbar again and end the worker.
    TurnOff,
    /// The setting is off and this process has not looked yet: undo a record an earlier host left, if there is one.
    StaleCheck,
    Nothing,
}

/// `already_looked` is whether this process has done the stale check (or started the worker, which does it).
pub fn decide(want: bool, running: bool, already_looked: bool) -> Action {
    match (want, running) {
        (true, false) => Action::Start,
        (false, true) => Action::TurnOff,
        (false, false) if !already_looked => Action::StaleCheck,
        _ => Action::Nothing,
    }
}

/// The settings file was (re)read, or the host started: brings hiding in line with the dock switch and the part's own value
/// (`dock::parts`).
pub fn on_settings(cx: &mut App) {
    let want = cx.try_global::<Settings>().is_some_and(|s| crate::dock::parts::runs(s, Part::Taskbar));
    match decide(want, cx.has_global::<Hiding>(), STALE_LOOKED.load(Ordering::SeqCst)) {
        Action::Start => {
            STALE_LOOKED.store(true, Ordering::SeqCst);
            start(cx);
        }
        Action::TurnOff => turn_off(cx),
        Action::StaleCheck => {
            STALE_LOOKED.store(true, Ordering::SeqCst);
            stale_check();
        }
        Action::Nothing => {}
    }
}

fn state_dir() -> Option<std::path::PathBuf> {
    crate::settings::path().and_then(|p| p.parent().map(std::path::Path::to_path_buf))
}

/// A host that crashed with the taskbar hidden leaves its record in `shell-state.json`. With the setting off nobody would look at
/// it, so it is looked at once, on a thread of its own (a file read, off the interface thread). Only when a record is there is the
/// worker started: it undoes the record as its first act, and ends without touching anything else.
fn stale_check() {
    let Some(dir) = state_dir() else { return };
    let spawned = std::thread::Builder::new().name("taskbar-stale-check".into()).spawn(move || {
        if !taskbar_hide::stale_record_present(&dir) {
            return;
        }
        log::warn!("taskbar hiding: the setting is off but an earlier run left a record of hidden windows; undoing it");
        let cfg = Config { state_dir: dir, guard_exe: taskbar_hide::guard_next_to_exe(), band_px: None, band_known: false };
        match TaskbarHider::start(cfg) {
            Ok(hider) => hider.finish(Duration::from_secs(10)),
            Err(e) => log::warn!("taskbar hiding: the earlier run's record could not be undone: {e}"),
        }
    });
    if let Err(e) = spawned {
        log::warn!("taskbar hiding: the stale-record check could not start: {e}");
    }
}

fn start(cx: &mut App) {
    let Some(state_dir) = state_dir() else {
        log::warn!("taskbar hiding: there is no folder for the state file; nothing is hidden");
        crate::dock::parts_status::note(Part::Taskbar, Some(parts::Why::RecordNotSaved));
        return;
    };
    // The bottom edge waits for the dock's word: a hider that starts before the dock has said what its band is would give the whole
    // monitor to maximised windows over the dock and set the work area a second time when the band arrives.
    let band = current_band();
    let cfg = Config { state_dir, guard_exe: taskbar_hide::guard_next_to_exe(), band_px: band.flatten(), band_known: band.is_some() };
    match TaskbarHider::start(cfg) {
        Ok(hider) => {
            hider.set_wanted(true);
            cx.set_global(Hiding { hider });
            crate::dock::parts_status::note(Part::Taskbar, None);
        }
        Err(e) => {
            log::warn!("taskbar hiding could not start: {e}");
            crate::dock::parts_status::note(Part::Taskbar, Some(parts::Why::NotStarted));
        }
    }
}

/// Whether the taskbar is shown again BEFORE the other parts are brought in line with the settings: only when it is being hidden now
/// and the settings no longer want it (the other order, hiding last, is for every other case). The rule is [`goes_first_when`].
pub fn goes_first(cx: &App) -> bool {
    let want = cx.try_global::<Settings>().is_some_and(|s| crate::dock::parts::runs(s, Part::Taskbar));
    goes_first_when(cx.has_global::<Hiding>(), want)
}

/// The order rule behind [`goes_first`]: hiding goes last (after the tray window and the keyboard hook exist), showing goes first.
pub fn goes_first_when(running: bool, want: bool) -> bool {
    running && !want
}

/// The fault that keeps the wanted hide from being in force right now (the guard missing, not confirming, lost; the record not
/// written), for the Dock page's line. `None` while nothing is wrong, and while the hide is not running at all.
pub fn fault(cx: &App) -> Option<taskbar_hide::Fault> {
    cx.try_global::<Hiding>().and_then(|h| h.hider.fault())
}

/// The setting went off: the taskbar comes back, off the interface thread.
fn turn_off(cx: &mut App) {
    crate::dock::parts_status::note(Part::Taskbar, None);
    let Hiding { hider } = cx.remove_global::<Hiding>();
    hider.set_wanted(false);
    std::thread::spawn(move || hider.stop(Duration::from_secs(5)));
}

/// The host is shutting down: the taskbar is shown again before anything else is torn down. Waits for the thread for three seconds
/// at most; what it did not finish is the guard's to finish.
pub fn stop(cx: &mut App) {
    if !cx.has_global::<Hiding>() {
        return;
    }
    let Hiding { hider } = cx.remove_global::<Hiding>();
    hider.stop(Duration::from_secs(3));
}

#[cfg(test)]
mod tests {
    use super::{Action, current_band, decide, note_band};
    use crate::settings::Settings;

    #[test]
    fn the_wiring_starts_hides_turns_off_and_looks_for_a_stale_record_once_when_the_setting_is_off() {
        assert_eq!(decide(true, false, false), Action::Start);
        assert_eq!(decide(true, false, true), Action::Start);
        assert_eq!(decide(true, true, false), Action::Nothing, "already running");
        assert_eq!(decide(false, true, true), Action::TurnOff);
        // the case the first review found missing: a crashed host's record with the setting OFF
        assert_eq!(decide(false, false, false), Action::StaleCheck, "the setting off at the first look: undo an earlier host's record");
        assert_eq!(decide(false, false, true), Action::Nothing, "and only once per process");
    }

    #[test]
    fn the_taskbar_is_shown_before_the_other_parts_go_and_hidden_after_they_are_in_place() {
        use super::goes_first_when;
        assert!(goes_first_when(true, false), "turning off: first");
        assert!(!goes_first_when(false, true), "turning on: last");
        assert!(!goes_first_when(true, true) && !goes_first_when(false, false), "no change: the order does not matter");
    }

    #[test]
    fn the_stale_check_starts_the_worker_only_for_a_state_file_that_holds_a_record() {
        // the decision itself is `rf_win::taskbar_hide::stale_record_present` (tested there against files); here it is checked that
        // the wiring uses it, and does so on a thread of its own and before anything is started
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell/taskbar_hide.rs")).unwrap().replace("\r\n", "\n");
        let body = &text[text.find("fn stale_check()").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let present = body.find("stale_record_present").expect("looks for a record");
        let started = body.find("TaskbarHider::start").expect("starts the worker");
        assert!(body.contains("std::thread::Builder") && present < started, "a thread, and the look comes before the start");
    }

    #[test]
    fn it_comes_with_the_dock_switch_unless_its_own_value_holds_it_off() {
        use super::{Part, parts::runs};
        assert!(!runs(&Settings::default(), Part::Taskbar), "the dock is off: Windows' taskbar is left alone");
        assert!(!runs(&Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap(), Part::Taskbar));
        let on = Settings { dock_enabled: true, ..Default::default() };
        assert!(runs(&on, Part::Taskbar));
        let back = Settings::parse_lenient_pub(&toml::to_string(&on).unwrap()).unwrap();
        assert!(runs(&back, Part::Taskbar));
        // the old file's false holds it off: nobody has the taskbar hidden by an update
        assert!(!runs(&Settings::parse_lenient_pub("dock_enabled = true\nshell_hide_taskbar = false\n").unwrap(), Part::Taskbar));
    }

    #[test]
    fn the_work_area_follows_the_switch_and_no_environment_variable_is_read() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell/taskbar_hide.rs")).unwrap();
        let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let before_tests = &code[..code.find("#[cfg(test)]").unwrap()];
        assert!(!before_tests.contains("env::var"), "the switch is the switch: no variable decides the work area any more");
        assert!(!before_tests.contains("RUSTYFILES_TASKBAR_WORKAREA"));
    }

    #[test]
    fn the_docks_band_is_remembered_and_only_a_change_is_passed_on() {
        // (one test touches the static: the tests of a binary share it)
        assert_eq!(current_band(), None, "before the dock has said anything: unknown, which is not the same as no band");
        assert!(note_band(Some(78)), "the first word is news");
        assert_eq!(current_band(), Some(Some(78)));
        assert!(!note_band(Some(78)), "the same again is not");
        assert!(note_band(Some(92)));
        assert!(note_band(None), "dock_reserve switched off is a change, too");
        assert_eq!(current_band(), Some(None), "said, and it is none");
        assert!(!note_band(None));
    }

    #[test]
    fn the_report_names_the_setting_only_when_it_is_on() {
        use crate::shell::probe::settings_on;
        assert!(!settings_on(&Settings::default()).contains(&"shell_hide_taskbar".to_string()));
        assert!(settings_on(&Settings { dock_enabled: true, ..Default::default() }).contains(&"shell_hide_taskbar".to_string()));
        assert!(!settings_on(&Settings { dock_enabled: true, shell_hide_taskbar: false, ..Default::default() }).contains(&"shell_hide_taskbar".to_string()));
    }

    #[test]
    fn this_file_changes_no_window_and_no_taskbar_setting_itself() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shell/taskbar_hide.rs")).unwrap();
        let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let before_tests = &code[..code.find("#[cfg(test)]").unwrap()];
        // (assembled from parts so that this test does not name them itself)
        for bad in [["Set", "WindowPos"].concat(), ["Show", "Window"].concat(), ["SHApp", "BarMessage"].concat(), ["System", "ParametersInfo"].concat(), ["Reg", "SetValue"].concat(), ["Send", "Message"].concat()] {
            assert!(!before_tests.contains(&bad), "taskbar_hide.rs names {bad}");
        }
    }
}
