//! The beside-guard against a pretend desktop, and the host and the guard together. Each way of LOSING the taskbar that the guard
//! has to answer is a named test: the host crashes, the host is killed, the power goes (a stale record), the guard is started
//! late, the host leaves cleanly.

use crate::beside::{EXIT_NOT_ARMED, EXIT_NOT_CONFIRMED, run_beside, run_beside_linked};
use crate::fakes::{FakeLink, World};
use crate::statefile::WorkAreaRec;
use crate::sys::{HostLink, PidWait};
use crate::tbctl::{Controller, Options};
use crate::tbfakes::*;
use crate::tbmodel::{HideRecord, PRIMARY, SECONDARY, Target};

const HOST_PID: u32 = 4242;
const HOST_START: u64 = 133_700_000_000_000_000;

fn world_with(desktop: &Desktop) -> World {
    let mut w = World::new(1_000_000);
    w.taskbar.desktop = Some(desktop.clone());
    w.procs.start_times.push((HOST_PID, HOST_START));
    w
}

fn count(w: &World, prefix: &str) -> usize {
    w.calls().iter().filter(|c| c.starts_with(prefix)).count()
}

fn exact(w: &World, call: &str) -> usize {
    w.calls().iter().filter(|c| *c == call).count()
}

fn target(h: isize, class: &str) -> Target {
    Target { handle: h, class: class.into(), pid: EXPLORER_PID, start: EXPLORER_START }
}

fn record(host_pid: u32, host_start: u64, handles: &[(isize, &str)]) -> HideRecord {
    HideRecord { host_pid, host_start, at_ms: 1, windows: handles.iter().map(|(h, c)| target(*h, c)).collect() }
}

fn hide_both(d: &Desktop, w: &mut World, host_pid: u32, host_start: u64) {
    for x in d.borrow_mut().iter_mut() {
        x.visible = false;
    }
    w.store.file.set_hidden_taskbar(&record(host_pid, host_start, &[(0x1001, PRIMARY), (0x1002, SECONDARY)]));
    w.store.file.set_pending_restore(true);
}

fn run(w: &mut World, link: Option<&mut FakeLink>) -> (i32, Vec<String>) {
    let mut lines = Vec::new();
    let code = {
        let mut log = |s: &str| lines.push(s.to_string());
        let mut sys = w.sys();
        match link {
            Some(l) => run_beside_linked(&mut sys, Some(l as &mut dyn HostLink), HOST_PID, &mut log),
            None => run_beside(&mut sys, HOST_PID, &mut log),
        }
    };
    (code, lines)
}

// ---------------------------------------------------------------- the host is lost

#[test]
fn the_host_crashes_with_the_taskbar_hidden_the_guard_shows_exactly_the_recorded_windows() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, HOST_PID, HOST_START);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (500, PidWait::Gone)]);
    let (code, _) = run(&mut w, None);
    assert_eq!(code, 0);
    assert_eq!(visible_handles(&d), vec![0x1001, 0x1002]);
    assert_eq!(count(&w, "taskbar.show_recorded"), 1);
    assert_eq!(exact(&w, "taskbar.show"), 0, "a record is there, so no sweep");
    assert!(!w.store.file.pending_restore() && !w.store.file.has_hidden_taskbar_key(), "flag and record are cleared");
    assert_eq!(count(&w, "registry."), 0);
    assert_eq!(count(&w, "procs.start_explorer"), 0);
}

#[test]
fn the_host_is_killed_in_task_manager_and_the_work_areas_come_back_before_the_record_is_cleared() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, HOST_PID, HOST_START);
    w.store.file.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1392] }]);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (500, PidWait::Gone)]);
    run(&mut w, None);
    assert_eq!(visible_handles(&d).len(), 2);
    assert_eq!(w.workarea.restored.len(), 1);
    assert!(w.pos("taskbar.show_recorded").unwrap() < w.pos("workarea.restore").unwrap());
}

#[test]
fn a_record_that_names_windows_of_another_owner_leaves_them_alone_and_the_guard_still_finishes() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    // Explorer was replaced since the record was written: the recorded owner is gone, and a window with the recorded number
    // belongs to somebody else now. It is not shown.
    {
        let mut dd = d.borrow_mut();
        dd[0].pid = 31337;
        dd[0].start = 77;
        dd[0].visible = false;
    }
    w.store.file.set_hidden_taskbar(&record(HOST_PID, HOST_START, &[(0x1001, PRIMARY)]));
    w.store.file.set_pending_restore(true);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    let (code, _) = run(&mut w, None);
    assert_eq!(code, 0);
    assert!(!d.borrow()[0].visible, "foreign window untouched");
    assert!(!w.store.file.has_hidden_taskbar_key());
}

#[test]
fn a_garbled_record_falls_back_to_showing_every_hidden_taskbar_of_explorer() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    for x in d.borrow_mut().iter_mut() {
        x.visible = false;
    }
    let dir = std::env::temp_dir().join(format!("rf-tbbeside-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = crate::statefile::path_in(&dir);
    std::fs::write(&p, r#"{"version":1,"hidden_taskbar":{"host_pid":"x"},"pending_restore":true}"#).unwrap();
    w.store.file = crate::statefile::ShellStateFile::load(&p);
    let _ = std::fs::remove_dir_all(&dir);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    let (code, _) = run(&mut w, None);
    assert_eq!(code, 0);
    assert_eq!(exact(&w, "taskbar.show"), 1, "the sweep ran once");
    assert_eq!(exact(&w, "taskbar.show_recorded"), 0);
    assert_eq!(visible_handles(&d).len(), 2);
}

#[test]
fn a_record_that_is_only_partly_readable_is_swept_whole_so_no_recorded_window_is_orphaned() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    for x in d.borrow_mut().iter_mut() {
        x.visible = false;
    }
    let dir = std::env::temp_dir().join(format!("rf-tbbeside-partly-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = crate::statefile::path_in(&dir);
    // the first entry is damaged, the second is fine: trimming would show one window, delete the key and forget the other
    std::fs::write(&p, r#"{"version":1,"hidden_taskbar":{"host_pid":4242,"windows":[{"hwnd":"x"},{"hwnd":4098,"class":"Shell_SecondaryTrayWnd","pid":8800,"start":133699000000000001}]},"pending_restore":true}"#).unwrap();
    w.store.file = crate::statefile::ShellStateFile::load(&p);
    let _ = std::fs::remove_dir_all(&dir);
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    let (code, _) = run(&mut w, None);
    assert_eq!(code, 0);
    assert_eq!(exact(&w, "taskbar.show"), 1, "the sweep ran, not the partial record");
    assert_eq!(exact(&w, "taskbar.show_recorded"), 0);
    assert_eq!(visible_handles(&d).len(), 2, "both windows are back, including the one the record could not name");
}

// ---------------------------------------------------------------- the power went / the guard starts late

#[test]
fn a_stale_record_of_an_earlier_host_is_undone_before_the_guard_says_it_is_armed() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, 777, 5); // another host hid them and is long gone
    let mut link = FakeLink::new(w.rec.clone());
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    let (code, _) = run(&mut w, Some(&mut link));
    assert_eq!(code, 0);
    let shown = w.pos("taskbar.show_recorded").unwrap();
    let armed = w.pos("link.confirm_ready").unwrap();
    assert!(shown < armed, "the host is told only after the old mess is cleaned: {:?}", w.calls());
    assert_eq!(visible_handles(&d).len(), 2);
}

#[test]
fn a_reused_process_id_makes_the_record_stale_too() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, HOST_PID, HOST_START - 99);
    let mut link = FakeLink::new(w.rec.clone());
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    run(&mut w, Some(&mut link));
    assert!(w.pos("taskbar.show_recorded").unwrap() < w.pos("link.confirm_ready").unwrap());
}

#[test]
fn a_record_of_this_very_host_is_not_stale_and_is_left_for_the_end() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, HOST_PID, HOST_START);
    let mut link = FakeLink::new(w.rec.clone());
    // alive, then told: the guard must not have touched the windows before
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Extra)]);
    run(&mut w, Some(&mut link));
    assert_eq!(count(&w, "taskbar."), 0);
}

// ---------------------------------------------------------------- the handshake

#[test]
fn the_guard_arms_then_confirms_then_waits_and_exits_quietly_when_told() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    let mut link = FakeLink::new(w.rec.clone());
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Extra)]);
    let (code, lines) = run(&mut w, Some(&mut link));
    assert_eq!(code, 0);
    assert!(w.pos("link.arm").unwrap() < w.pos("link.confirm_ready").unwrap());
    assert_eq!(count(&w, "taskbar."), 0, "told: nothing is shown, nothing is looked at");
    assert!(lines.iter().any(|l| l.contains("armed")));
}

#[test]
fn told_means_done_even_if_a_record_has_already_been_written_for_the_next_guard() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    // setting off then on again at once: the host wrote a new record before this (old) guard looked
    hide_both(&d, &mut w, HOST_PID, HOST_START);
    let mut link = FakeLink::new(w.rec.clone());
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Extra)]);
    let (code, _) = run(&mut w, Some(&mut link));
    assert_eq!(code, 0);
    assert_eq!(visible_handles(&d), Vec::<isize>::new(), "the new guard's hiding is not undone by the old guard");
    assert!(w.store.file.hidden_taskbar().is_some());
}

#[test]
fn events_that_cannot_be_opened_mean_no_confirmation_and_an_exit() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    let mut link = FakeLink::new(w.rec.clone());
    link.arm_ok = false;
    let (code, _) = run(&mut w, Some(&mut link));
    assert_eq!(code, EXIT_NOT_ARMED);
    assert_eq!(count(&w, "link.confirm_ready"), 0);
}

#[test]
fn a_confirmation_that_cannot_be_sent_ends_the_guard_without_pretending() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    let mut link = FakeLink::new(w.rec.clone());
    link.confirm_ok = false;
    w.procs.pid_waits.extend([(0, PidWait::Timeout)]);
    let (code, _) = run(&mut w, Some(&mut link));
    assert_eq!(code, EXIT_NOT_CONFIRMED);
}

#[test]
fn a_host_that_is_already_dead_when_the_guard_starts_is_not_confirmed_and_the_taskbar_is_shown() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, HOST_PID, HOST_START);
    let mut link = FakeLink::new(w.rec.clone());
    // pid_waits empty: the fake answers Gone
    let (code, _) = run(&mut w, Some(&mut link));
    assert_eq!(code, 0);
    assert_eq!(count(&w, "link.confirm_ready"), 0, "nobody is there to confirm to");
    assert_eq!(visible_handles(&d).len(), 2);
}

#[test]
fn the_session_ending_leaves_everything_as_it_is() {
    let d = two_monitor_desktop();
    let mut w = world_with(&d);
    hide_both(&d, &mut w, HOST_PID, HOST_START);
    let mut link = FakeLink::new(w.rec.clone());
    w.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    w.session.ending.push_back(true);
    let (code, _) = run(&mut w, Some(&mut link));
    assert_eq!(code, 0);
    assert_eq!(count(&w, "taskbar."), 0);
}

// ---------------------------------------------------------------- host and guard together

/// The host hides through the controller; the host then "dies" (the controller is dropped without a word); the guard runs.
#[test]
fn host_and_guard_together_a_crash_of_the_host_after_hiding_brings_the_taskbar_back() {
    let d = two_monitor_desktop();
    let mut hw = FakeWorld::new(d.clone());
    let mut c = Controller::new(HOST_PID, HOST_START, Options { give_back_work_area: false });
    c.set_wanted(&mut hw, true);
    assert_eq!(visible_handles(&d), Vec::<isize>::new());
    // the host dies here: no restore, no word to the guard. The guard sees the file the host left.
    let mut gw = world_with(&d);
    gw.store.file = hw.state.clone();
    gw.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Gone)]);
    let (code, _) = run(&mut gw, None);
    assert_eq!(code, 0);
    assert_eq!(visible_handles(&d), vec![0x1001, 0x1002]);
    assert!(!gw.store.file.has_hidden_taskbar_key());
}

#[test]
fn host_and_guard_together_a_clean_exit_leaves_nothing_for_the_guard_to_do() {
    let d = two_monitor_desktop();
    let mut hw = FakeWorld::new(d.clone());
    let mut c = Controller::new(HOST_PID, HOST_START, Options { give_back_work_area: false });
    c.set_wanted(&mut hw, true);
    c.quit(&mut hw);
    assert!(hw.guard_done);
    assert_eq!(visible_handles(&d).len(), 2);
    let mut gw = world_with(&d);
    gw.store.file = hw.state.clone();
    let mut link = FakeLink::new(gw.rec.clone());
    gw.procs.pid_waits.extend([(0, PidWait::Timeout), (0, PidWait::Extra)]);
    let (code, _) = run(&mut gw, Some(&mut link));
    assert_eq!(code, 0);
    assert_eq!(count(&gw, "taskbar."), 0);
}
