//! The whole guard against fakes: the runner, the machine and the state file together, with a
//! scripted host and a shared record of every call so the order across traits can be asserted.

use std::path::PathBuf;

use crate::beside::run_beside;
use crate::fakes::World;
use crate::machine::{Crash, Message, Outcome, Persist, Reason};
use crate::runner::{RunConfig, RunReport, run_guard};
use crate::statefile::{ShellStateFile, WorkAreaRec};
use crate::sys::{HostLaunch, PidWait, StateStore, Sys, Wait};

fn cfg(panic: bool) -> RunConfig {
    RunConfig { launch: HostLaunch { exe: PathBuf::from("C:\\app\\rustyfiles.exe"), args: vec!["--shell".into()], ready_event: "Local\\ready".into() }, panic }
}

/// A world whose state file already knows this build, so the self-test does not run.
fn world() -> World {
    let mut w = World::new(1_000_000);
    w.store.file.apply(&Persist::Ready { build: Some("26300.9974".into()), crashes: vec![] });
    w
}

fn go(w: &mut World, panic: bool) -> (RunReport, Vec<String>) {
    let mut lines = Vec::new();
    let report = {
        let mut log = |s: &str| lines.push(s.to_string());
        let mut sys = w.sys();
        run_guard(&mut sys, &cfg(panic), &mut log)
    };
    (report, lines)
}

fn count(w: &World, prefix: &str) -> usize {
    w.calls().iter().filter(|c| c.starts_with(prefix)).count()
}

fn ready(pid: u32) -> (u64, Wait) {
    let _ = pid;
    (0, Wait::HostReady)
}

#[test]
fn run_then_escape_returns_to_explorer_in_the_planned_order() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (1000, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    w.procs.explorer_up.extend([false, true]); // probe: not up; wait: up
    let (report, _) = go(&mut w, false);
    assert_eq!(report, RunReport { outcome: Some(Outcome::FellBack(Reason::Escape)), exit_code: 0 });

    let calls = w.calls();
    let p = |prefix: &str| w.pos(prefix).unwrap_or_else(|| panic!("{prefix} was never called: {calls:?}"));
    assert!(p("hotkey.arm") < p("procs.spawn_host"));
    assert!(p("session.create_shell_ready_event") < p("procs.spawn_host"));
    assert!(p("procs.spawn_host") < p("session.signal_shell_ready"));
    assert!(p("session.release_identity") < p("registry.delete_shell_value"), "identity goes first");
    assert!(p("registry.delete_shell_value") < p("procs.start_explorer"), "the value goes before Explorer starts: {calls:?}");
    assert!(p("procs.kill_host") < p("procs.start_explorer"));
    assert!(p("workarea.restore") < p("procs.start_explorer"));

    assert!(w.registry.shell.is_none());
    assert!(w.ui.messages.is_empty());
    let f = &w.store.file;
    assert_eq!(f.fallback_reason(), Some(Reason::Escape));
    assert_eq!(f.value_cleared(), Some(true));
    assert_eq!(f.unresolved(), None);
}

#[test]
fn a_host_that_keeps_crashing_ends_in_explorer_and_the_switch_turns_off() {
    let mut w = world();
    w.procs.waits.extend([(0, Wait::HostExited { pid: 101, code: 1 }), (0, Wait::HostExited { pid: 102, code: 1 })]);
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::CrashLoop)));
    assert_eq!(count(&w, "procs.spawn_host"), 2);
    assert_eq!(count(&w, "taskbar.show"), 2, "after the first crash, and once in the fall-back that the second crash goes straight into");
    assert!(w.registry.shell.is_none());
    assert_eq!(w.store.file.fallback_reason(), Some(Reason::CrashLoop));
    assert!(w.store.file.crashes().is_empty(), "a cleared switch starts with a clean log");
    assert_eq!(count(&w, "procs.start_explorer"), 1);
}

#[test]
fn a_host_that_cannot_be_started_at_all_ends_in_explorer() {
    let mut w = world();
    w.procs.spawn.extend([Err("not found".into()), Err("not found".into())]);
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::CrashLoop)));
    assert_eq!(count(&w, "procs.spawn_host"), 2);
    assert!(w.registry.shell.is_none());
}

#[test]
fn backoff_goes_through_the_runner_and_the_fake_clock() {
    let mut w = world();
    w.procs.waits.extend([
        ready(101),
        (20_000, Wait::HostExited { pid: 101, code: 1 }), // crash 1: relaunched at once
        ready(102),
        (20_000, Wait::HostExited { pid: 102, code: 1 }), // crash 2: one second of backoff
        (1_000, Wait::Timeout),                           // the backoff ends
        ready(103),
        (5_000, Wait::Extra), // the user escapes
    ]);
    w.hotkey.pressed.push_back(true);
    let before = w.time.get();
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)));
    assert_eq!(count(&w, "procs.spawn_host"), 3);
    assert_eq!(w.time.get() - before, 20_000 + 20_000 + 1_000 + 5_000);
}

#[test]
fn a_hung_host_is_killed_and_restarted() {
    let mut w = world();
    w.procs.waits.extend([
        ready(101),
        (10_000, Wait::Timeout),
        (10_000, Wait::Timeout),
        ready(102),
        (1_000, Wait::Extra),
    ]);
    w.procs.hung.extend([true, true]);
    w.hotkey.pressed.push_back(true);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)));
    assert!(count(&w, "procs.kill_host") >= 2, "once for the hang, once in the fall-back: {:?}", w.calls());
    assert_eq!(count(&w, "procs.spawn_host"), 2);
}

#[test]
fn the_host_asking_to_leave_returns_to_explorer() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (3_600_000, Wait::HostExited { pid: 101, code: crate::machine::EXIT_LEAVE })]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::UserExit)));
    assert!(w.registry.shell.is_none());
    assert_eq!(count(&w, "procs.start_explorer"), 0, "Explorer was reported up already by the default fake");
}

#[test]
fn a_host_that_exits_zero_while_the_user_signs_out_leaves_the_switch_alone() {
    // Windows closes the host's windows at sign-out and the host answers with exit code 0. That must not turn the
    // switch off: the guard sees the session ending and does nothing at all.
    let mut w = world();
    w.procs.waits.extend([ready(101), (3_600_000, Wait::HostExited { pid: 101, code: 0 })]);
    w.session.ending.push_back(true);
    let (report, _) = go(&mut w, false);
    assert_eq!(report, RunReport { outcome: Some(Outcome::SessionEnded), exit_code: 0 });
    assert!(w.registry.shell.is_some(), "the Shell value stays");
    assert_eq!(count(&w, "registry."), 0);
    assert_eq!(count(&w, "procs.start_explorer"), 0);
    assert_eq!(count(&w, "procs.spawn_host"), 1, "and the host is not started again");
}

#[test]
fn a_crash_while_the_user_signs_out_is_not_a_crash() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (1_000, Wait::HostExited { pid: 101, code: 1 })]);
    w.session.ending.push_back(true);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::SessionEnded));
    assert!(w.registry.shell.is_some());
    assert!(w.store.file.crashes().is_empty());
}

#[test]
fn a_host_that_exits_zero_on_its_own_is_started_again_and_the_switch_stays_on() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (3_600_000, Wait::HostExited { pid: 101, code: 0 }), ready(102), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)), "only the escape chord ended it");
    assert_eq!(count(&w, "procs.spawn_host"), 2);
    assert_eq!(count(&w, "registry.delete_shell_value"), 1, "once, in the fall-back at the end");
    assert!(w.pos("procs.spawn_host").unwrap() < w.pos("registry.delete_shell_value").unwrap());
}

#[test]
fn explorer_already_the_shell_stops_the_guard_before_it_starts_a_host() {
    let mut w = world();
    w.procs.explorer_is_shell = true;
    let (report, _) = go(&mut w, false);
    assert_eq!(report, RunReport { outcome: Some(Outcome::ExplorerHasShell), exit_code: 0 });
    assert_eq!(count(&w, "procs.spawn_host"), 0);
    assert_eq!(count(&w, "session.create_shell_ready_event"), 0, "Explorer's ShellReadyEvent is not touched either");
    assert!(w.registry.shell.is_some(), "the value stays: this run was not the one that decides");
    assert_eq!(count(&w, "procs.start_explorer"), 0);
}

#[test]
fn panic_does_not_ask_whether_explorer_is_the_shell() {
    let mut w = world();
    w.procs.explorer_is_shell = true;
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, true);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Panic)));
    assert_eq!(count(&w, "procs.explorer_owns_shell_window"), 0);
    assert!(w.registry.shell.is_none());
}

#[test]
fn the_host_pid_is_kept_in_the_state_file_only_while_the_host_runs() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (1_000, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    go(&mut w, false);
    // Escape ended it: the host was killed in the fall-back and its id forgotten.
    assert_eq!(w.store.file.host_pid(), None);

    // While it ran, the file named it: a store that looks at every write sees the id at the first save after the spawn.
    struct Spy {
        inner: crate::fakes::MemStore,
        seen: Vec<Option<u32>>,
    }
    impl StateStore for Spy {
        fn load(&mut self) -> ShellStateFile {
            self.inner.load()
        }
        fn save(&mut self, f: &ShellStateFile) -> Result<(), String> {
            self.seen.push(f.host_pid());
            self.inner.save(f)
        }
    }
    let mut w = world();
    w.procs.waits.extend([ready(101), (1_000, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    let mut spy = Spy { inner: crate::fakes::MemStore::new(w.store.file.clone()), seen: vec![] };
    run_with_store(&mut w, &mut spy, false);
    assert!(spy.seen.contains(&Some(101)), "{:?}", spy.seen);
    assert_eq!(spy.seen.last().copied().flatten(), None);
}

/// `go`, but with a store of the test's own.
fn run_with_store(w: &mut World, store: &mut dyn StateStore, panic: bool) -> RunReport {
    let mut log = |_: &str| {};
    let mut sys = Sys {
        clock: &w.clock,
        registry: &mut w.registry,
        procs: &mut w.procs,
        hotkey: &mut w.hotkey,
        session: &mut w.session,
        workarea: &mut w.workarea,
        taskbar: &mut w.taskbar,
        store,
        ui: &mut w.ui,
    };
    run_guard(&mut sys, &cfg(panic), &mut log)
}

#[test]
fn a_standalone_panic_ends_the_host_the_dead_guard_left() {
    let mut w = world();
    w.store.file.set_host_pid(Some(4242));
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, true);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Panic)));
    let calls = w.calls();
    let p = |prefix: &str| w.pos(prefix).unwrap_or_else(|| panic!("{prefix} missing: {calls:?}"));
    assert!(p("procs.adopt_host 4242") < p("procs.kill_host"));
    assert!(p("procs.kill_host") < p("procs.start_explorer"), "the host's window is gone before Explorer starts");
    assert_eq!(w.store.file.host_pid(), None);
}

#[test]
fn a_panic_that_was_handed_to_a_running_guard_adopts_nothing() {
    let mut w = world();
    w.store.file.set_host_pid(Some(4242));
    w.session.delegate_ok = true;
    go(&mut w, true);
    assert_eq!(count(&w, "procs.adopt_host"), 0);
    assert_eq!(w.store.file.host_pid(), Some(4242), "the running guard owns that host");
}

#[test]
fn no_host_pid_in_the_file_means_nothing_is_adopted() {
    let mut w = world();
    w.procs.explorer_up.extend([false, true]);
    go(&mut w, true);
    assert_eq!(count(&w, "procs.adopt_host"), 0);
}

#[test]
fn what_the_host_wrote_after_the_guard_started_survives_the_guards_writes_and_is_restored() {
    // The host shrinks the work area and sets the pending flag AFTER the guard loaded the file. The guard's own writes
    // (ready, crash log) must not erase that, and when the host then dies the rectangles are put back.
    struct HostWrites {
        inner: crate::fakes::MemStore,
        loads: usize,
    }
    impl StateStore for HostWrites {
        fn load(&mut self) -> ShellStateFile {
            self.loads += 1;
            // The first load is the guard's start; the host does its writing after that.
            if self.loads == 2 {
                self.inner.file.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }]);
                self.inner.file.set_pending_restore(true);
            }
            self.inner.load()
        }
        fn save(&mut self, f: &ShellStateFile) -> Result<(), String> {
            self.inner.save(f)
        }
    }
    let mut w = world();
    w.procs.waits.extend([ready(101), (50_000, Wait::HostExited { pid: 101, code: 1 }), ready(102), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    let mut store = HostWrites { inner: crate::fakes::MemStore::new(w.store.file.clone()), loads: 0 };
    run_with_store(&mut w, &mut store, false);
    let first_restore = &w.workarea.restored[0];
    assert_eq!(first_restore, &vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }], "restored from what the host wrote, not from the copy read at start");
    assert!(!store.inner.file.pending_restore(), "cleared after the restore");
    assert_eq!(store.inner.file.original_work_areas().len(), 1, "the host's keys are still there");
}

#[test]
fn a_write_of_the_guard_keeps_keys_the_host_added_in_between() {
    struct HostAdds {
        inner: crate::fakes::MemStore,
        loads: usize,
    }
    impl StateStore for HostAdds {
        fn load(&mut self) -> ShellStateFile {
            self.loads += 1;
            if self.loads == 2 {
                self.inner.file.set_host_exe("C:\\app\\rustyfiles.exe");
                self.inner.file.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 10, 10] }]);
            }
            self.inner.load()
        }
        fn save(&mut self, f: &ShellStateFile) -> Result<(), String> {
            self.inner.save(f)
        }
    }
    let mut w = world();
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    let mut store = HostAdds { inner: crate::fakes::MemStore::new(w.store.file.clone()), loads: 0 };
    run_with_store(&mut w, &mut store, false);
    assert_eq!(store.inner.file.host_exe(), Some("C:\\app\\rustyfiles.exe"));
    assert_eq!(store.inner.file.original_work_areas().len(), 1);
}

#[test]
fn a_planned_restart_does_not_count_and_does_not_touch_explorer() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (60_000, Wait::HostExited { pid: 101, code: crate::machine::EXIT_RESTART }), ready(102), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)));
    assert_eq!(count(&w, "procs.spawn_host"), 2);
}

#[test]
fn a_failed_self_test_never_starts_the_host() {
    let mut w = World::new(0); // no last known good build: the self-test runs
    w.procs.self_test = false;
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::SelfTestFailed)));
    assert_eq!(count(&w, "procs.spawn_host"), 0);
    assert!(w.registry.shell.is_none());
    assert!(w.pos("procs.host_self_test").is_some());
}

#[test]
fn the_self_test_passes_once_and_is_not_repeated_for_the_same_build() {
    let mut w = World::new(0);
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    go(&mut w, false);
    assert_eq!(count(&w, "procs.host_self_test"), 1);
    assert_eq!(w.store.file.last_known_good_build(), Some("26300.9974"));
}

// ---- safe mode, shift, panic ---------------------------------------------------------------

#[test]
fn safe_mode_starts_explorer_and_leaves_the_value() {
    let mut w = world();
    w.session.safe_mode = true;
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::SafeMode)));
    assert!(w.registry.shell.is_some(), "the value stays for the next normal sign-in");
    assert_eq!(count(&w, "registry.delete"), 0);
    assert_eq!(count(&w, "procs.spawn_host"), 0);
    assert_eq!(count(&w, "procs.start_explorer"), 1);
    assert_eq!(w.store.file.unresolved(), None);
}

#[test]
fn shift_at_sign_in_skips_the_shell_once() {
    let mut w = world();
    w.session.shift = true;
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::UserSkip)));
    assert!(w.registry.shell.is_some());
    assert_eq!(count(&w, "procs.start_explorer"), 1);
}

#[test]
fn panic_deletes_the_value_and_starts_explorer() {
    let mut w = world();
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, true);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Panic)));
    assert!(w.registry.shell.is_none());
    assert_eq!(count(&w, "procs.start_explorer"), 1);
    assert_eq!(count(&w, "hotkey.arm"), 0, "a panic process arms nothing");
    assert!(w.pos("session.signal_running_guard_panic").is_some(), "it asks a running guard first");
}

#[test]
fn panic_in_safe_mode_still_deletes_the_value() {
    let mut w = world();
    w.session.safe_mode = true;
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, true);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Panic)));
    assert!(w.registry.shell.is_none());
}

#[test]
fn panic_handed_to_a_running_guard_does_nothing_itself() {
    let mut w = world();
    w.session.delegate_ok = true;
    let (report, _) = go(&mut w, true);
    assert_eq!(report, RunReport { outcome: Some(Outcome::Delegated), exit_code: 0 });
    assert!(w.registry.shell.is_some());
    assert_eq!(count(&w, "procs.start_explorer"), 0, "two Explorer starts would open a folder window");
    assert_eq!(count(&w, "registry.delete"), 0);
}

#[test]
fn the_session_ending_exits_quietly() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.session.ending.push_back(true);
    let (report, _) = go(&mut w, false);
    assert_eq!(report, RunReport { outcome: Some(Outcome::SessionEnded), exit_code: 0 });
    assert!(w.registry.shell.is_some());
    assert_eq!(count(&w, "procs.start_explorer"), 0);
}

#[test]
fn a_spurious_wake_of_the_extra_handle_is_ignored() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (1, Wait::Extra), (1, Wait::Extra)]);
    w.hotkey.pressed.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)));
}

// ---- when clearing the value fails ----------------------------------------------------------

#[test]
fn a_value_that_cannot_be_cleared_is_reported_and_retried_at_the_next_start_without_a_second_explorer() {
    let mut w = world();
    w.registry.fail_delete = true;
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)));
    assert_eq!(w.ui.messages, vec![Message { value_left: true, explorer_down: false }]);
    assert_eq!(count(&w, "procs.start_explorer"), 1, "the user still gets a desktop");
    assert_eq!(w.store.file.unresolved(), Some(Reason::Escape));
    assert!(w.registry.shell.is_some());

    // Winlogon starts the guard again at the next sign-in (or restarts it). This time the delete works.
    w.registry.fail_delete = false;
    w.ui.messages.clear();
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Escape)));
    assert!(w.registry.shell.is_none());
    assert_eq!(w.store.file.unresolved(), None);
    assert_eq!(count(&w, "procs.spawn_host"), 1, "the second start never launched the host");
    assert_eq!(count(&w, "procs.start_explorer"), 1, "Explorer was up already: not started a second time");
    assert!(w.ui.messages.is_empty());
}

#[test]
fn an_unresolved_record_is_ignored_when_the_value_is_gone() {
    let mut w = world();
    w.store.file.apply(&Persist::FallbackStart { reason: Reason::Escape, crashes: vec![] });
    w.registry.shell = None; // the user ran the repair line
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    let (_, lines) = go(&mut w, false);
    assert_eq!(count(&w, "procs.spawn_host"), 1, "the guard carries on normally: {lines:?}");
}

#[test]
fn explorer_that_never_starts_gives_a_message_and_exit_code_one() {
    let mut w = world();
    w.procs.explorer_up.extend([false, false, false]);
    w.procs.explorer_start.extend([true, true]);
    let (report, _) = go(&mut w, true);
    assert_eq!(report, RunReport { outcome: Some(Outcome::FellBack(Reason::Panic)), exit_code: 1 });
    assert_eq!(w.ui.messages, vec![Message { value_left: false, explorer_down: true }]);
    assert_eq!(count(&w, "procs.start_explorer"), 2);
}

#[test]
fn work_areas_saved_by_the_host_are_put_back_when_the_host_dies() {
    let mut w = world();
    w.procs.waits.extend([ready(101), (50_000, Wait::HostExited { pid: 101, code: 1 }), ready(102), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    go(&mut w, false);
    // The crash path asked the work-area side to restore; with no saved rectangles it was handed an empty list.
    assert!(!w.workarea.restored.is_empty() && w.workarea.restored.iter().all(|r| r.is_empty()));
    assert!(w.pos("workarea.restore").is_some());
}

#[test]
fn a_pending_restore_in_the_state_file_is_carried_out_once_and_cleared() {
    let dir = std::env::temp_dir().join(format!("rf-shellguard-test-{}-run-pending", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shell-state.json");
    std::fs::write(
        &path,
        r#"{"version":1,"pending_restore":true,"last_known_good_build":"26300.9974","original_work_areas":[{"monitor":"A","rect":[0,0,1920,1040]},{"monitor":"B","rect":[-1080,-200,0,1720]}]}"#,
    )
    .unwrap();
    let mut w = World::new(5_000_000);
    w.store.file = ShellStateFile::load(&path);
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    go(&mut w, false);
    assert_eq!(w.workarea.restored[0], vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }, WorkAreaRec { monitor: "B".into(), rect: [-1080, -200, 0, 1720] }]);
    assert!(!w.store.file.pending_restore(), "cleared once it was done");
    assert!(w.pos("workarea.restore").unwrap() < w.pos("procs.spawn_host").unwrap(), "undone before the new host starts");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_state_file_means_a_restore_is_assumed_pending() {
    let dir = std::env::temp_dir().join(format!("rf-shellguard-test-{}-run-corrupt", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shell-state.json");
    std::fs::write(&path, b"{ this is not json").unwrap();
    let mut w = World::new(0);
    w.store.file = ShellStateFile::load(&path);
    w.procs.waits.extend([ready(101), (1, Wait::Extra)]);
    w.hotkey.pressed.push_back(true);
    go(&mut w, false);
    assert!(w.pos("workarea.restore").is_some());
    assert!(w.pos("taskbar.show").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_state_file_from_a_newer_version_does_not_stop_the_way_back() {
    let dir = std::env::temp_dir().join(format!("rf-shellguard-test-{}-run-newer", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shell-state.json");
    std::fs::write(&path, r#"{"version":9,"crashes":[]}"#).unwrap();
    let mut w = World::new(0);
    w.store.file = ShellStateFile::load(&path);
    assert!(!w.store.file.writable);
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, true);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::Panic)));
    assert!(w.registry.shell.is_none());
    assert_eq!(w.store.saves, 0, "nothing was written over the newer file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crashes_logged_by_an_earlier_guard_count_in_this_one() {
    let mut w = world();
    let now = w.time.get();
    w.store.file.apply(&Persist::Crashes(vec![Crash { at: now - 20_000, uptime_ms: 5_000 }, Crash { at: now - 10_000, uptime_ms: 5_000 }]));
    w.procs.waits.extend([(0, Wait::HostExited { pid: 101, code: 1 })]);
    w.procs.explorer_up.extend([false, true]);
    let (report, _) = go(&mut w, false);
    assert_eq!(report.outcome, Some(Outcome::FellBack(Reason::CrashLoop)), "the third crash inside a minute, across guard restarts");
}

// ---- beside mode --------------------------------------------------------------------------

fn beside(w: &mut World, pid: u32) -> (i32, Vec<String>) {
    let mut lines = Vec::new();
    let code = {
        let mut log = |s: &str| lines.push(s.to_string());
        let mut sys = w.sys();
        run_beside(&mut sys, pid, &mut log)
    };
    (code, lines)
}

#[test]
fn beside_guard_shows_the_taskbar_when_the_host_dies_and_clears_the_flag() {
    let mut w = world();
    w.store.file.set_pending_restore(true);
    w.procs.pid_waits.extend([(0, PidWait::Timeout) /* alive at start */, (500, PidWait::Gone)]);
    let (code, _) = beside(&mut w, 4242);
    assert_eq!(code, 0);
    assert_eq!(count(&w, "taskbar.show"), 1);
    assert!(!w.store.file.pending_restore());
    // It does nothing else: no registry, no Explorer, no host.
    assert_eq!(count(&w, "registry."), 0);
    assert_eq!(count(&w, "procs.start_explorer"), 0);
    assert_eq!(count(&w, "procs.spawn_host"), 0);
}

#[test]
fn beside_guard_puts_the_work_areas_back_before_it_clears_the_shared_flag() {
    let mut w = world();
    w.store.file.set_pending_restore(true);
    w.store.file.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }]);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (500, PidWait::Gone)]);
    let (code, _) = beside(&mut w, 4242);
    assert_eq!(code, 0);
    assert_eq!(w.workarea.restored, vec![vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }]]);
    assert!(w.pos("taskbar.show").unwrap() < w.pos("workarea.restore").unwrap());
    assert!(!w.store.file.pending_restore());
    assert_eq!(w.store.file.original_work_areas().len(), 1, "the rectangles stay for the host's next run");
}

#[test]
fn beside_guard_keeps_the_flag_when_the_work_areas_cannot_be_put_back() {
    let mut w = world();
    w.store.file.set_pending_restore(true);
    w.store.file.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }]);
    w.workarea.fail = true;
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (500, PidWait::Gone)]);
    let (_, lines) = beside(&mut w, 4242);
    assert!(w.store.file.pending_restore(), "so that the next start tries again");
    assert!(lines.iter().any(|l| l.contains("work areas not restored")));
}

#[test]
fn beside_guard_with_nothing_pending_touches_no_work_area() {
    let mut w = world();
    w.store.file.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }]);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (500, PidWait::Gone)]);
    beside(&mut w, 4242);
    assert_eq!(count(&w, "workarea.restore"), 0, "the host never said it changed one");
}

#[test]
fn beside_guard_with_a_host_already_dead_acts_at_once() {
    let mut w = world();
    // pid_waits empty: the fake answers Gone.
    let (code, _) = beside(&mut w, 4242);
    assert_eq!(code, 0);
    assert_eq!(count(&w, "taskbar.show"), 1);
}

#[test]
fn beside_guard_waits_for_explorer_to_come_back_and_gives_up_in_the_end() {
    let mut w = world();
    w.store.file.set_pending_restore(true);
    w.taskbar.results.extend([Err("no".to_string()), Ok(0), Ok(1)]);
    let before = w.time.get();
    let (code, _) = beside(&mut w, 4242);
    assert_eq!(code, 0);
    assert_eq!(count(&w, "taskbar.show"), 3);
    assert_eq!(w.time.get() - before, 2 * crate::beside::RETRY_MS, "a second between tries");

    let mut w = world();
    w.store.file.set_pending_restore(true);
    w.taskbar.results.extend(std::iter::repeat_n(Err("no".to_string()), 10));
    let (code, _) = beside(&mut w, 4242);
    assert_eq!(code, 1);
    assert_eq!(count(&w, "taskbar.show"), crate::beside::RETRIES as usize);
    assert!(w.store.file.pending_restore(), "the flag stays so the next start tries again");
}

#[test]
fn beside_guard_exits_quietly_when_the_session_ends() {
    let mut w = world();
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    w.session.ending.push_back(true);
    let (code, _) = beside(&mut w, 4242);
    assert_eq!(code, 0);
    assert_eq!(count(&w, "taskbar.show"), 0);
}
