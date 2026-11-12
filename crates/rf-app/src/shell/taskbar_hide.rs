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
//! WHAT IS CHANGED, and what is not: only the visibility of Explorer's taskbar windows. NOT the auto-hide state or any other
//! taskbar setting (the call that sets it writes the user's taskbar settings, and that it is undone on every path could not be
//! shown without running it), not the registry, not the work area unless `RUSTYFILES_TASKBAR_WORKAREA=1` is set (a first try is
//! made without it; see docs/design/taskbar-hide.md).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::*;
use rf_win::taskbar_hide::{self, Config, TaskbarHider};

use crate::settings::Settings;

/// Asks for the work area to be given back as well (a second step of the first careful try; see the design note).
pub const ENV_WORK_AREA: &str = "RUSTYFILES_TASKBAR_WORKAREA";

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

/// The settings file was (re)read, or the host started: brings hiding in line with `shell_hide_taskbar`.
pub fn on_settings(cx: &mut App) {
    let want = cx.try_global::<Settings>().is_some_and(|s| s.shell_hide_taskbar);
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
        let cfg = Config { state_dir: dir, guard_exe: taskbar_hide::guard_next_to_exe(), give_back_work_area: false };
        match TaskbarHider::start(cfg) {
            Ok(hider) => hider.finish(Duration::from_secs(10)),
            Err(e) => log::warn!("taskbar hiding: the earlier run's record could not be undone: {e}"),
        }
    });
    if let Err(e) = spawned {
        log::warn!("taskbar hiding: the stale-record check could not start: {e}");
    }
}

/// What the configuration says, from the environment of this process (pure apart from reading it).
pub fn work_area_wanted(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
}

fn start(cx: &mut App) {
    let Some(state_dir) = state_dir() else {
        log::warn!("taskbar hiding: there is no folder for the state file; nothing is hidden");
        return;
    };
    let cfg = Config {
        state_dir,
        guard_exe: taskbar_hide::guard_next_to_exe(),
        give_back_work_area: work_area_wanted(std::env::var(ENV_WORK_AREA).ok().as_deref()),
    };
    match TaskbarHider::start(cfg) {
        Ok(hider) => {
            hider.set_wanted(true);
            cx.set_global(Hiding { hider });
        }
        Err(e) => log::warn!("taskbar hiding could not start: {e}"),
    }
}

/// The setting went off: the taskbar comes back, off the interface thread.
fn turn_off(cx: &mut App) {
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
    use super::{Action, decide, work_area_wanted};
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
    fn it_is_off_by_default_and_a_wrong_value_is_off() {
        assert!(!Settings::default().shell_hide_taskbar, "experimental: off until asked for");
        assert!(!Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap().shell_hide_taskbar);
        assert!(!Settings::parse_lenient_pub("shell_hide_taskbar = \"evet\"\n").unwrap().shell_hide_taskbar, "a wrong value falls back to off");
        let on = Settings { shell_hide_taskbar: true, ..Default::default() };
        let back = Settings::parse_lenient_pub(&toml::to_string(&on).unwrap()).unwrap();
        assert!(back.shell_hide_taskbar);
    }

    #[test]
    fn the_work_area_step_is_asked_for_only_by_a_one() {
        assert!(!work_area_wanted(None));
        assert!(!work_area_wanted(Some("")));
        assert!(!work_area_wanted(Some("0")));
        assert!(!work_area_wanted(Some("true")));
        assert!(work_area_wanted(Some("1")));
        assert!(work_area_wanted(Some(" 1 ")));
    }

    #[test]
    fn the_report_names_the_setting_only_when_it_is_on() {
        use crate::shell::probe::settings_on;
        assert!(!settings_on(&Settings::default()).contains(&"shell_hide_taskbar".to_string()));
        assert!(settings_on(&Settings { shell_hide_taskbar: true, ..Default::default() }).contains(&"shell_hide_taskbar".to_string()));
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
