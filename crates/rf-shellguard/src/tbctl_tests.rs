//! The controller against a pretend desktop. Each way of LOSING the taskbar, and each way of getting it back, is a named test.

use crate::tbctl::{Controller, RESPAWN_BACKOFF_MS, RESTORE_RETRY_BASE_MS, RESTORE_RETRY_LIMIT, Status};
use crate::tbfakes::*;
use crate::tbmodel::{GuardStatus, HideRecord, PRIMARY, Refusal, SECONDARY, Target};

const HOST_PID: u32 = 4242;
const HOST_START: u64 = 133_700_000_000_000_000;

fn ctl() -> Controller {
    Controller::new(HOST_PID, HOST_START)
}

fn world() -> FakeWorld {
    FakeWorld::new(two_monitor_desktop())
}

fn on(c: &mut Controller, w: &mut FakeWorld) {
    c.set_wanted(w, true);
}

fn target(h: isize, class: &str) -> Target {
    Target { handle: h, class: class.into(), pid: EXPLORER_PID, start: EXPLORER_START }
}

// ---------------------------------------------------------------- hiding

#[test]
fn switching_on_starts_the_guard_waits_for_it_records_and_only_then_hides() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    assert_eq!(visible_handles(&w.desktop), Vec::<isize>::new(), "both taskbar windows are hidden");
    assert_eq!(c.status(), Status::Hidden(2));
    let order = ["guard.start", "guard.ready", "save", "hide:1001"];
    let at: Vec<usize> = order.iter().map(|p| w.pos(p).unwrap_or_else(|| panic!("{p} missing in {:?}", w.calls))).collect();
    assert!(at.windows(2).all(|p| p[0] < p[1]), "{:?}", w.calls);
    // the record on disk names both windows, the host and the pending flag
    let rec = w.state.hidden_taskbar().unwrap();
    assert_eq!((rec.host_pid, rec.host_start), (HOST_PID, HOST_START));
    assert_eq!(rec.windows, vec![target(0x1001, PRIMARY), target(0x1002, SECONDARY)]);
    assert!(w.state.pending_restore());
}

#[test]
fn the_record_is_on_disk_before_each_hide_not_after_the_last() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    let save = w.pos("save").unwrap();
    assert!(save < w.pos("hide:1001").unwrap() && save < w.pos("hide:1002").unwrap());
}

#[test]
fn nothing_is_hidden_when_the_guard_is_missing() {
    let (mut c, mut w) = (ctl(), world());
    w.guard_exists = false;
    on(&mut c, &mut w);
    assert_eq!(w.count("hide:"), 0);
    assert_eq!(w.count("save"), 0, "no record either: nothing changed");
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardMissing));
    assert_eq!(visible_handles(&w.desktop).len(), 2);
}

#[test]
fn nothing_is_hidden_when_the_guard_does_not_start() {
    let (mut c, mut w) = (ctl(), world());
    w.guard_start_ok = false;
    on(&mut c, &mut w);
    assert_eq!((w.count("hide:"), w.count("save")), (0, 0));
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardNotConfirmed));
}

#[test]
fn nothing_is_hidden_when_the_guard_does_not_confirm_and_it_is_ended() {
    let (mut c, mut w) = (ctl(), world());
    w.guard_ready_ok = false;
    on(&mut c, &mut w);
    assert_eq!((w.count("hide:"), w.count("save")), (0, 0));
    assert!(w.guard_killed && !w.guard_running, "a guard that never confirmed is not left running");
    assert_eq!(c.guard_status(), GuardStatus::Absent);
}

#[test]
fn nothing_is_hidden_when_the_state_file_cannot_be_written() {
    let (mut c, mut w) = (ctl(), world());
    w.save_fails = true;
    on(&mut c, &mut w);
    assert_eq!(w.count("hide:"), 0, "no record, no hiding");
    assert_eq!(c.status(), Status::Waiting(Refusal::RecordNotSaved));
    assert_eq!(visible_handles(&w.desktop).len(), 2);
}

#[test]
fn nothing_is_hidden_when_explorer_is_not_the_shell_and_no_guard_is_started() {
    let (mut c, mut w) = (ctl(), world());
    w.explorer_shell = false;
    on(&mut c, &mut w);
    assert_eq!((w.count("hide:"), w.count("guard.start")), (0, 0));
    assert_eq!(c.status(), Status::Waiting(Refusal::NoExplorerShell));
}

#[test]
fn nothing_is_hidden_when_explorer_has_no_visible_taskbar_yet_and_a_later_tick_does_it() {
    let (mut c, mut w) = (ctl(), world());
    let saved: Vec<_> = w.desktop.borrow_mut().drain(..).collect();
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::NothingToHide));
    *w.desktop.borrow_mut() = saved;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Hidden(2));
}

#[test]
fn a_taskbar_that_was_already_hidden_by_something_else_is_not_ours_to_show() {
    let (mut c, mut w) = (ctl(), world());
    w.desktop.borrow_mut()[1].visible = false;
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(1));
    c.set_wanted(&mut w, false);
    assert_eq!(visible_handles(&w.desktop), vec![0x1001], "the primary came back; the other one stays as it was");
    assert_eq!(w.count("show:1002"), 0);
}

#[test]
fn a_window_that_cannot_be_hidden_is_not_counted() {
    let (mut c, mut w) = (ctl(), world());
    w.hide_fails = vec![0x1002];
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(1));
}

// ---------------------------------------------------------------- getting it back

#[test]
fn the_setting_going_off_shows_the_taskbar_clears_the_record_and_then_tells_the_guard() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    c.set_wanted(&mut w, false);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert!(w.state.hidden_taskbar().is_none() && !w.state.has_hidden_taskbar_key() && !w.state.pending_restore());
    assert!(w.guard_done && !w.guard_killed);
    assert!(w.pos("show:1001").unwrap() < w.pos("guard.done").unwrap(), "the guard is told after the windows are back");
    assert_eq!(c.status(), Status::Off);
}

#[test]
fn quitting_the_host_does_the_same() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    c.quit(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert!(w.guard_done);
    assert!(!w.state.has_hidden_taskbar_key());
}

#[test]
fn a_window_that_cannot_be_shown_keeps_the_record_and_leaves_the_guard_to_finish() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.show_fails = vec![0x1002];
    c.quit(&mut w);
    assert!(w.state.hidden_taskbar().is_some(), "so that the guard (or the next start) tries again");
    assert!(!w.guard_done && w.guard_running, "the guard is not told it can go");
}

#[test]
fn switching_off_when_nothing_was_ever_hidden_touches_nothing() {
    let (mut c, mut w) = (ctl(), world());
    c.set_wanted(&mut w, false);
    c.quit(&mut w);
    assert_eq!(w.calls.len(), 0, "{:?}", w.calls);
}

#[test]
fn the_guard_ending_while_hidden_brings_the_taskbar_back_at_the_next_tick_and_hiding_stops() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.guard_running = false; // killed by someone
    c.tick(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2, "shown again at once");
    // and it is not hidden again while no guard is confirmed
    w.guard_start_ok = false;
    w.now += RESPAWN_BACKOFF_MS[0] + 1;
    c.tick(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardNotConfirmed));
}

#[test]
fn a_lost_guard_is_replaced_after_a_pause_and_hiding_resumes() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.guard_running = false;
    c.tick(&mut w);
    assert_eq!(w.guard_starts, 1);
    w.now += 1_000;
    c.tick(&mut w);
    assert_eq!(w.guard_starts, 1, "not started again within the pause");
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    w.now += RESPAWN_BACKOFF_MS[0];
    c.tick(&mut w);
    assert_eq!(w.guard_starts, 2);
    assert_eq!(c.status(), Status::Hidden(2));
}

#[test]
fn a_guard_that_keeps_dying_waits_longer_each_time() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    for (round, pause) in RESPAWN_BACKOFF_MS.iter().enumerate() {
        w.guard_running = false;
        c.tick(&mut w); // lost: shown, pause starts
        let starts = w.guard_starts;
        w.now += pause - 1;
        c.tick(&mut w);
        assert_eq!(w.guard_starts, starts, "round {round}: still pausing");
        w.now += 1;
        c.tick(&mut w);
        assert_eq!(w.guard_starts, starts + 1, "round {round}: tried again");
    }
}

#[test]
fn the_controllers_startup_undoes_a_record_left_by_a_crashed_host_whatever_the_setting_is() {
    let (mut c, mut w) = (ctl(), world());
    // an earlier host (other pid) hid both windows and died
    let old = HideRecord { host_pid: 777, host_start: 5, at_ms: 1, windows: vec![target(0x1001, PRIMARY), target(0x1002, SECONDARY)] };
    for h in [0x1001, 0x1002] {
        w.desktop.borrow_mut().iter_mut().find(|x| x.handle == h).unwrap().visible = false;
    }
    w.state.set_hidden_taskbar(&old);
    w.state.set_pending_restore(true);
    c.startup(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert!(!w.state.has_hidden_taskbar_key() && !w.state.pending_restore());
    assert_eq!(w.count("guard.start"), 0, "no guard for this: the setting is off");
}

#[test]
fn a_record_of_a_reused_process_id_is_also_stale() {
    let (mut c, mut w) = (ctl(), world());
    let old = HideRecord { host_pid: HOST_PID, host_start: HOST_START - 1, at_ms: 1, windows: vec![target(0x1001, PRIMARY)] };
    w.desktop.borrow_mut()[0].visible = false;
    w.state.set_hidden_taskbar(&old);
    c.startup(&mut w);
    assert_eq!(visible_handles(&w.desktop), vec![0x1001, 0x1002]);
}

#[test]
fn after_a_reboot_the_old_handles_are_gone_and_nothing_foreign_is_touched() {
    let (mut c, mut w) = (ctl(), world());
    // the record names windows of the previous Windows session: a new Explorer (other pid and start) owns other windows now,
    // and one of the old handle numbers happens to belong to a new Explorer window
    let old = HideRecord {
        host_pid: 777,
        host_start: 5,
        at_ms: 1,
        windows: vec![Target { handle: 0x1001, class: PRIMARY.into(), pid: 1111, start: 42 }, Target { handle: 0x9999, class: SECONDARY.into(), pid: 1111, start: 42 }],
    };
    w.desktop.borrow_mut()[0].visible = false; // hidden by somebody else: not ours (the record's owner differs)
    w.state.set_hidden_taskbar(&old);
    c.startup(&mut w);
    assert_eq!(w.count("show:"), 0, "{:?}", w.calls);
    assert_eq!(visible_handles(&w.desktop), vec![0x1002]);
    assert!(!w.state.has_hidden_taskbar_key(), "the stale record is gone");
}

#[test]
fn an_unreadable_record_shows_every_hidden_explorer_taskbar() {
    let (mut c, mut w) = (ctl(), world());
    for x in w.desktop.borrow_mut().iter_mut() {
        x.visible = false;
    }
    // a record that parses as nothing
    let d = std::env::temp_dir().join(format!("rf-tbctl-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let p = crate::statefile::path_in(&d);
    std::fs::write(&p, r#"{"version":1,"hidden_taskbar":"???","pending_restore":true}"#).unwrap();
    w.state = crate::statefile::ShellStateFile::load(&p);
    let _ = std::fs::remove_dir_all(&d);
    c.startup(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert!(!w.state.has_hidden_taskbar_key());
}

#[test]
fn explorer_restarting_forgets_the_old_handles_and_hides_the_new_taskbar_when_still_wanted() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    // Explorer is replaced: new process, new windows, the old ones are gone
    let newbie = |h, class: &str, r| {
        let mut x = window(h, class, r);
        x.pid = 9100;
        x.start = EXPLORER_START + 77;
        x
    };
    *w.desktop.borrow_mut() = vec![newbie(0x2001, PRIMARY, [0, 1392, 2560, 1440]), newbie(0x2002, SECONDARY, [-1440, 2311, 0, 2359])];
    c.explorer_restarted(&mut w);
    assert_eq!(visible_handles(&w.desktop), Vec::<isize>::new());
    assert_eq!(c.status(), Status::Hidden(2));
    // the old handles are never shown (they do not exist; had they been reused, the owner check would stop it)
    assert_eq!(w.count("show:"), 0);
    // the record names the new windows as well
    let rec = w.state.hidden_taskbar().unwrap();
    assert!(rec.windows.iter().any(|t| t.handle == 0x2001 && t.pid == 9100));
}

#[test]
fn explorer_restarting_with_the_setting_off_does_nothing() {
    let (mut c, mut w) = (ctl(), world());
    c.explorer_restarted(&mut w);
    assert_eq!(w.calls.len(), 0);
}

#[test]
fn a_new_monitor_gets_its_secondary_taskbar_recorded_and_then_hidden() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.calls.clear();
    w.desktop.borrow_mut().push(window(0x1003, SECONDARY, [1920, 1040, 3840, 1080]));
    c.displays_changed(&mut w);
    assert!(w.pos("save").unwrap() < w.pos("hide:1003").unwrap(), "record first: {:?}", w.calls);
    assert_eq!(c.status(), Status::Hidden(3));
    assert_eq!(w.state.hidden_taskbar().unwrap().windows.len(), 3);
}

#[test]
fn a_monitor_that_went_away_leaves_a_harmless_entry_that_is_skipped_at_restore() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.desktop.borrow_mut().retain(|x| x.handle != 0x1002);
    c.quit(&mut w);
    assert_eq!(w.count("show:1002"), 0);
    assert!(w.guard_done, "a window that no longer exists is not a failure");
}

#[test]
fn explorer_showing_the_taskbar_again_is_hidden_again_at_the_next_tick() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.desktop.borrow_mut()[0].visible = true;
    c.tick(&mut w);
    assert_eq!(visible_handles(&w.desktop), Vec::<isize>::new());
}

#[test]
fn a_pid_reused_by_another_program_window_is_not_hidden_or_shown() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    // the primary window's handle now belongs to a window of another process (same class by chance, not Explorer)
    {
        let mut d = w.desktop.borrow_mut();
        d[0].pid = 31337;
        d[0].start = 99;
        d[0].explorer = false;
        d[0].visible = true;
    }
    c.quit(&mut w);
    assert_eq!(w.count("show:1001"), 0, "{:?}", w.calls);
    assert_eq!(visible_handles(&w.desktop), vec![0x1001, 0x1002]);
}

#[test]
fn the_guard_is_not_started_for_a_session_without_a_taskbar_even_when_asked_twice() {
    let (mut c, mut w) = (ctl(), world());
    w.explorer_shell = false;
    on(&mut c, &mut w);
    on(&mut c, &mut w);
    assert_eq!(w.guard_starts, 0);
}

// ---------------------------------------------------------------- a flaky world

#[test]
fn a_listing_that_fails_hides_nothing_and_changes_no_state() {
    let (mut c, mut w) = (ctl(), world());
    w.list_fails = true;
    on(&mut c, &mut w);
    assert_eq!((w.count("hide:"), w.count("save")), (0, 0));
}

#[test]
fn hiding_twice_is_idempotent() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    let hides = w.count("hide:");
    on(&mut c, &mut w);
    c.tick(&mut w);
    assert_eq!(w.count("hide:"), hides, "windows that are hidden are not hidden again");
    assert_eq!(w.guard_starts, 1);
}

// ---------------------------------------------------------------- a show that is only asked for, and a restore that is owed

#[test]
fn a_show_that_is_accepted_but_never_happens_is_not_a_restore_the_record_and_the_guard_stay() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    // Explorer is hung: SetWindowPos (queued) returned Ok, nothing happens
    w.show_lost = vec![0x1001, 0x1002];
    c.quit(&mut w);
    assert_eq!(visible_handles(&w.desktop), Vec::<isize>::new(), "still hidden");
    assert!(w.state.hidden_taskbar().is_some() && w.state.pending_restore(), "the record is NOT removed on the strength of an Ok");
    assert!(!w.guard_done && w.guard_running, "the guard is not told it can go");
    assert!(c.restore_pending());
}

#[test]
fn a_show_that_takes_effect_while_it_is_waited_for_is_a_restore() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.show_late = vec![0x1001, 0x1002];
    c.set_wanted(&mut w, false);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert!(w.state.hidden_taskbar().is_none() && w.guard_done && !c.restore_pending());
    assert!(w.pos("show:1001").unwrap() < w.pos("wait_visible").unwrap() && w.pos("wait_visible").unwrap() < w.pos("guard.done").unwrap());
}

#[test]
fn the_setting_off_with_a_lost_show_is_retried_on_later_ticks_until_it_works() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.show_lost = vec![0x1001];
    c.set_wanted(&mut w, false);
    assert!(c.restore_pending() && !w.guard_done);
    // Explorer comes back to life: the next try works. The setting is off, and the retry does not care.
    w.show_lost.clear();
    w.now += RESTORE_RETRY_BASE_MS - 1;
    c.tick(&mut w);
    assert!(c.restore_pending(), "the pause is not over yet");
    w.now += 1;
    c.tick(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert!(!c.restore_pending() && w.guard_done, "done, so the guard is told it can go");
    assert!(w.state.hidden_taskbar().is_none() && !w.state.pending_restore());
    assert_eq!(c.status(), Status::Off);
}

#[test]
fn the_retries_wait_longer_each_time_and_stop_after_the_limit_with_the_guard_still_there() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.show_fails = vec![0x1002];
    c.quit(&mut w);
    let mut tries = 0;
    let mut last_gap = 0;
    let mut gap = 0u64;
    for _ in 0..200_000 {
        if !c.restore_pending() {
            break;
        }
        let before = w.count("show:1002");
        w.now += 500;
        gap += 500;
        c.tick(&mut w);
        if w.count("show:1002") > before {
            tries += 1;
            assert!(gap >= last_gap, "the pause grows: {gap} after {last_gap}");
            last_gap = gap;
            gap = 0;
        }
    }
    assert!(!c.restore_pending(), "gave up");
    assert_eq!(tries, RESTORE_RETRY_LIMIT, "{:?}", w.calls);
    assert!(w.state.hidden_taskbar().is_some() && !w.guard_done && w.guard_running, "the record and the guard are what is left");
}

#[test]
fn a_lost_guard_with_a_restore_that_fails_is_retried_before_anything_is_hidden_again() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.guard_running = false;
    w.show_lost = vec![0x1001, 0x1002];
    c.tick(&mut w);
    assert!(c.restore_pending(), "the show did not happen");
    assert_eq!(visible_handles(&w.desktop).len(), 0);
    // later ticks: the strategy is to restore first. Once Explorer answers, the windows come back; only then may a new guard
    // (after its pause) hide them again.
    w.show_lost.clear();
    w.now += RESTORE_RETRY_BASE_MS;
    c.tick(&mut w);
    assert_eq!(visible_handles(&w.desktop).len(), 2, "shown by the retry although no guard was confirmed");
    assert!(!c.restore_pending());
    assert_eq!(w.guard_starts, 1, "no new guard yet: its pause is not over");
}

#[test]
fn a_restore_that_is_run_twice_touches_nothing_the_second_time() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    c.set_wanted(&mut w, false);
    let calls = w.calls.len();
    c.quit(&mut w);
    c.set_wanted(&mut w, false);
    c.tick(&mut w);
    assert_eq!(w.calls.len(), calls, "{:?}", &w.calls[calls..]);
}

#[test]
fn the_panic_net_and_the_timer_know_when_a_restore_is_owed() {
    let (mut c, mut w) = (ctl(), world());
    assert!(!c.holds_windows() && !c.restore_pending());
    on(&mut c, &mut w);
    assert!(c.holds_windows());
    w.show_lost = vec![0x1001];
    c.set_wanted(&mut w, false);
    assert!(c.holds_windows() && c.restore_pending(), "still owed after the setting went off");
    w.show_lost.clear();
    w.now += RESTORE_RETRY_BASE_MS;
    c.tick(&mut w);
    assert!(!c.holds_windows() && !c.restore_pending());
}

// ---------------------------------------------------------------- what the status says while a guard is being waited for

#[test]
fn a_guard_that_is_missing_keeps_saying_so_through_the_pauses_between_looks() {
    let (mut c, mut w) = (ctl(), world());
    w.guard_exists = false;
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardMissing));
    // the pauses are 5, 15 and then 60 seconds; a pass inside one must not turn "not found" into "lost" (nothing was ever running)
    for pause in RESPAWN_BACKOFF_MS {
        w.now += pause / 2;
        c.tick(&mut w);
        assert_eq!(c.status(), Status::Waiting(Refusal::GuardMissing), "inside the {pause} ms pause");
        w.now += pause;
        c.tick(&mut w);
        assert_eq!(c.status(), Status::Waiting(Refusal::GuardMissing), "at the look after the {pause} ms pause");
    }
    assert_eq!(w.count("hide:"), 0);
    // the guard turns up: the next look after the pause starts it and the taskbar is hidden
    w.guard_exists = true;
    w.now += RESPAWN_BACKOFF_MS[2] + 1;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Hidden(2));
}

#[test]
fn a_guard_that_did_not_confirm_keeps_saying_so_through_the_pause() {
    let (mut c, mut w) = (ctl(), world());
    w.guard_ready_ok = false;
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardNotConfirmed));
    w.now += RESPAWN_BACKOFF_MS[0] / 2;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardNotConfirmed), "inside the pause");
}

#[test]
fn a_guard_that_ended_says_lost_through_the_pause_and_then_what_the_next_try_finds() {
    let (mut c, mut w) = (ctl(), world());
    on(&mut c, &mut w);
    w.guard_running = false;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardLost));
    w.now += 1_000;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardLost), "still the pause after the loss");
    // the replacement cannot be found: from then on that is the reason
    w.guard_exists = false;
    w.now += RESPAWN_BACKOFF_MS[0];
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardMissing));
    w.now += 1_000;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardMissing), "and it stays the reason inside the next pause");
}
