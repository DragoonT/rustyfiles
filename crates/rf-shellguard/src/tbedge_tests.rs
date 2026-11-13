//! The bottom edge while our hide of the taskbar is in force: the controller against a pretend desktop that also models what Explorer
//! does with the work area. Named tests for the order of the steps on the way in and on every way out, and for "nothing is set while
//! the hide is not confirmed".

use crate::statefile::WorkAreaRec;
use crate::tbctl::{Controller, EDGE_DOCK_WAIT_MS, EDGE_RETRY_MS, Edge, RESTORE_RETRY_BASE_MS, Status};
use crate::tbfakes::*;
use crate::tbmodel::{GuardStatus, MonitorGeom, PRIMARY, Refusal, SECONDARY};
use std::cell::RefCell;
use std::rc::Rc;

const HOST_PID: u32 = 4242;
const HOST_START: u64 = 133_700_000_000_000_000;
const BAND: i32 = 78;

const A: [i32; 4] = [0, 0, 2560, 1440];
const B: [i32; 4] = [-1440, -200, 0, 2359];

/// The user's PC: a 2560x1440 primary monitor with a 48 px taskbar on the bottom edge and the dock's 78 px band registered with
/// Explorer (work area 2560x1314), and a portrait monitor to its left with its own taskbar.
fn world(dock_reserve: bool) -> (Controller, FakeWorld) {
    let band = dock_reserve.then_some(BAND);
    let c = Controller::new(HOST_PID, HOST_START).with_band(band);
    let mut w = FakeWorld::new(two_monitor_desktop());
    w.monitors = vec![
        MonitorGeom { id: "A".into(), rect: A, work: [0, 0, 2560, 1392 - band.unwrap_or(0)], primary: true },
        MonitorGeom { id: "B".into(), rect: B, work: [-1440, -200, 0, 2311], primary: false },
    ];
    w.dock_band = band;
    w.dock_wants_band = band;
    (c, w)
}

fn on(c: &mut Controller, w: &mut FakeWorld) {
    c.set_wanted(w, true);
}

fn work_a(w: &FakeWorld) -> [i32; 4] {
    w.monitors[0].work
}

fn work_b(w: &FakeWorld) -> [i32; 4] {
    w.monitors[1].work
}

/// The positions of these calls, each searched for AFTER the one before; panics with the whole log when one is missing or out of order.
fn in_order(w: &FakeWorld, steps: &[&str]) {
    let mut from = 0;
    for s in steps {
        match w.calls[from..].iter().position(|c| c.starts_with(s)) {
            Some(i) => from += i + 1,
            None => panic!("{s} is missing, or comes before the step before it, in {:?}", w.calls),
        }
    }
}

// ---------------------------------------------------------------- the way in

#[test]
fn the_edge_is_taken_in_a_fixed_order_and_the_dock_floats_at_the_bottom_of_the_monitor() {
    let (mut c, mut w) = world(true);
    assert_eq!(work_a(&w), [0, 0, 2560, 1314], "the trial's before-picture: the strip and the band");
    on(&mut c, &mut w);
    in_order(
        &w,
        &[
            "guard.start",
            "guard.ready",
            "save",         // the record of the hidden windows
            "hide:1001",    // the hide
            "wait_hidden",  // ... confirmed
            "save",         // the user's own work areas, written down before the first change
            "dock.take:0,1362,2560,1440",
            "dock.ready?",
            "work:0,0,2560,1362",
        ],
    );
    assert_eq!(c.status(), Status::Hidden(2));
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362], "the work area ends where the dock's band begins: no 48 px strip under the dock");
    assert_eq!(work_b(&w), B, "the other monitor: the whole monitor");
    assert_eq!(w.dock_edge, Some([0, 1362, 2560, 1440]), "the dock's band is the bottom strip of the monitor");
    // the originals are what the user has WITHOUT the dock's band, and pending_restore is set
    assert_eq!(
        w.state.original_work_areas(),
        vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1392] }, WorkAreaRec { monitor: "B".into(), rect: [-1440, -200, 0, 2311] }]
    );
    assert!(w.state.pending_restore() && w.state.hidden_taskbar().is_some());
}

#[test]
fn without_dock_reserve_the_whole_monitor_is_given_and_the_islands_still_sit_on_the_edge() {
    let (mut c, mut w) = world(false);
    assert_eq!(work_a(&w), [0, 0, 2560, 1392]);
    on(&mut c, &mut w);
    assert_eq!(work_a(&w), A);
    assert_eq!(work_b(&w), B);
    assert_eq!(w.dock_edge, Some([0, 1440, 2560, 1440]), "an empty strip at the very bottom");
    assert_eq!(w.state.original_work_areas()[0].rect, [0, 0, 2560, 1392]);
}

#[test]
fn the_user_s_work_areas_are_written_down_before_the_first_change_the_dock_letting_go_included() {
    let (mut c, mut w) = world(true);
    on(&mut c, &mut w);
    let saves: Vec<usize> = w.calls.iter().enumerate().filter(|(_, x)| *x == "save").map(|(i, _)| i).collect();
    assert_eq!(saves.len(), 2, "{:?}", w.calls);
    assert!(saves[1] < w.pos("dock.take").unwrap(), "{:?}", w.calls);
    assert!(saves[1] < w.pos("work:").unwrap());
}

#[test]
fn nothing_is_set_while_the_hide_is_not_confirmed() {
    // Explorer is busy: the queued hide is accepted and never happens
    let (mut c, mut w) = world(true);
    w.hide_lost = vec![0x1001, 0x1002];
    on(&mut c, &mut w);
    assert!(w.pos("hide:1001").is_some() && w.pos("wait_hidden").is_some());
    assert_eq!((w.count("dock."), w.count("work:")), (0, 0), "{:?}", w.calls);
    assert!(w.state.original_work_areas().is_empty(), "the work areas are not even written down");
    assert_eq!((c.edge(), work_a(&w), w.dock_edge), (Edge::Free, [0, 0, 2560, 1314], None), "the dock keeps its band");
    assert!(!matches!(c.status(), Status::Hidden(_)));
    // Explorer comes back to life: the next pass hides them, the hide is confirmed, and only then the edge is taken
    w.hide_lost.clear();
    w.now += 1_000;
    c.tick(&mut w);
    assert_eq!(c.status(), Status::Hidden(2));
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

#[test]
fn a_hide_that_takes_effect_while_it_is_waited_for_is_a_confirmed_hide() {
    let (mut c, mut w) = world(true);
    w.hide_late = vec![0x1001, 0x1002];
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(2));
    assert!(w.pos("wait_hidden").unwrap() < w.pos("dock.take").unwrap());
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

#[test]
fn only_the_confirmed_windows_count_and_only_their_monitors_are_touched() {
    let (mut c, mut w) = world(true);
    w.hide_lost = vec![0x1002]; // the second monitor's taskbar stays visible
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(1));
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
    assert_eq!(work_b(&w), [-1440, -200, 0, 2311], "its taskbar is still there: its strip is its own");
}

#[test]
fn a_hide_that_is_refused_a_missing_guard_and_an_unwritable_record_set_nothing() {
    for what in ["refused", "no guard", "guard silent", "no record", "not the shell"] {
        let (mut c, mut w) = world(true);
        match what {
            "refused" => w.hide_fails = vec![0x1001, 0x1002],
            "no guard" => w.guard_exists = false,
            "guard silent" => w.guard_ready_ok = false,
            "no record" => w.save_fails = true,
            _ => w.explorer_shell = false,
        }
        on(&mut c, &mut w);
        assert_eq!((w.count("dock."), w.count("work:")), (0, 0), "{what}: {:?}", w.calls);
        assert_eq!((c.edge(), work_a(&w)), (Edge::Free, [0, 0, 2560, 1314]), "{what}");
        assert!(w.dock_edge.is_none() && w.state.original_work_areas().is_empty(), "{what}");
    }
}

#[test]
fn originals_that_cannot_be_written_mean_the_dock_is_not_touched_and_the_try_is_made_again_later() {
    let (mut c, mut w) = world(true);
    w.save_fails_after = Some(1); // the record is written, the originals are not
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(2), "the taskbar is hidden; only the edge waits");
    assert_eq!((w.count("dock."), w.count("work:"), c.edge()), (0, 0, Edge::Free));
    // before the pause is over nothing happens
    w.save_fails_after = None;
    w.now += EDGE_RETRY_MS[0] - 1;
    c.tick(&mut w);
    assert_eq!(w.count("dock."), 0);
    w.now += 1;
    c.tick(&mut w);
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

// ---------------------------------------------------------------- the dock lets go

#[test]
fn a_dock_that_is_slow_to_let_go_is_waited_for_and_no_work_area_is_set_before_it_has() {
    let (mut c, mut w) = world(true);
    w.dock_release_after = 2;
    on(&mut c, &mut w);
    assert!(matches!(c.edge(), Edge::Releasing { .. }) && c.edge_waiting());
    assert_eq!(w.count("work:"), 0);
    assert_eq!(work_a(&w), [0, 0, 2560, 1314], "Explorer still has the dock's band");
    w.now += 100;
    c.tick(&mut w);
    assert_eq!(w.count("work:"), 0, "{:?}", w.calls);
    w.now += 100;
    c.tick(&mut w);
    assert_eq!(c.edge(), Edge::Ours);
    assert!(!c.edge_waiting());
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
    assert!(w.pos("dock.take").unwrap() < w.pos("work:").unwrap());
}

#[test]
fn a_dock_that_never_lets_go_gets_its_band_back_and_is_asked_again_after_a_pause() {
    let (mut c, mut w) = world(true);
    w.dock_stuck = true;
    on(&mut c, &mut w);
    assert_eq!(w.count("work:"), 0);
    w.now += EDGE_DOCK_WAIT_MS;
    c.tick(&mut w);
    assert_eq!(c.edge(), Edge::Free);
    assert_eq!((w.count("dock.give"), w.count("work:")), (1, 0), "the dock has its band again, nothing was set");
    assert_eq!(work_a(&w), [0, 0, 2560, 1314]);
    assert_eq!(c.status(), Status::Hidden(2), "the taskbar stays hidden; the strip stays too, as before this increment");
    // the pause, then the dock lets go
    w.dock_stuck = false;
    w.now += EDGE_RETRY_MS[0] - 1;
    c.tick(&mut w);
    assert_eq!(w.count("dock.take"), 1);
    w.now += 1;
    c.tick(&mut w);
    assert_eq!(w.count("dock.take"), 2);
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

// ---------------------------------------------------------------- the way out

fn settled() -> (Controller, FakeWorld) {
    let (mut c, mut w) = world(true);
    on(&mut c, &mut w);
    assert_eq!(c.edge(), Edge::Ours);
    w.calls.clear();
    (c, w)
}

#[test]
fn switching_off_puts_things_back_in_a_fixed_order_and_the_dock_registers_its_band_again() {
    let (mut c, mut w) = settled();
    c.set_wanted(&mut w, false);
    in_order(
        &w,
        &[
            "show:1001",
            "wait_visible",             // the taskbar is back, confirmed
            "work:0,0,2560,1392",       // the user's own work areas, from the written-down rectangles
            "work:-1440,-200,0,2311",
            "dock.give",                // the dock registers its band with Explorer again
            "save",                     // the record and the originals are removed
            "guard.done",               // and only then the guard is told
        ],
    );
    assert_eq!(visible_handles(&w.desktop).len(), 2);
    assert_eq!((c.edge(), c.status()), (Edge::Free, Status::Off));
    assert_eq!(work_a(&w), [0, 0, 2560, 1314], "the taskbar's strip and the dock's band, as before");
    assert_eq!(work_b(&w), [-1440, -200, 0, 2311]);
    assert!(w.state.hidden_taskbar().is_none() && !w.state.pending_restore() && w.state.original_work_areas().is_empty());
}

#[test]
fn quitting_does_the_same_in_the_same_order() {
    let (mut c, mut w) = settled();
    c.quit(&mut w);
    in_order(&w, &["show:1001", "wait_visible", "work:0,0,2560,1392", "dock.give", "save", "guard.done"]);
    assert_eq!(work_a(&w), [0, 0, 2560, 1314]);
    assert!(w.guard_done && w.dock_edge.is_none());
}

#[test]
fn a_show_that_never_happens_leaves_the_edge_the_dock_and_the_guard_as_they_are() {
    let (mut c, mut w) = settled();
    w.show_lost = vec![0x1001];
    c.set_wanted(&mut w, false);
    assert_eq!((w.count("work:"), w.count("dock.give")), (0, 0), "nothing is put back for a taskbar that is not back: {:?}", w.calls);
    assert_eq!(c.edge(), Edge::Ours);
    assert!(c.restore_pending() && !w.guard_done && w.guard_running);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
    // Explorer answers later: the whole way out, in order
    w.show_lost.clear();
    w.now += RESTORE_RETRY_BASE_MS;
    c.tick(&mut w);
    assert!(!c.restore_pending() && w.guard_done);
    assert_eq!(c.edge(), Edge::Free);
    assert_eq!(work_a(&w), [0, 0, 2560, 1314]);
}

#[test]
fn work_areas_that_cannot_be_put_back_keep_the_record_the_edge_and_the_guard() {
    let (mut c, mut w) = settled();
    w.work_fails = true;
    c.quit(&mut w);
    assert!(w.state.hidden_taskbar().is_some() && w.state.pending_restore() && !w.state.original_work_areas().is_empty());
    assert_eq!((c.edge(), w.count("dock.give")), (Edge::Ours, 0));
    assert!(!w.guard_done);
    w.work_fails = false;
    w.now += RESTORE_RETRY_BASE_MS;
    c.tick(&mut w);
    assert_eq!((c.edge(), w.guard_done), (Edge::Free, true));
    assert!(w.state.hidden_taskbar().is_none());
}

#[test]
fn a_guard_that_ended_brings_everything_back_in_order_and_hiding_stops() {
    let (mut c, mut w) = settled();
    w.guard_running = false;
    c.tick(&mut w);
    in_order(&w, &["show:1001", "wait_visible", "work:0,0,2560,1392", "dock.give", "save"]);
    assert_eq!((c.edge(), c.guard_status()), (Edge::Free, GuardStatus::Lost));
    assert_eq!(work_a(&w), [0, 0, 2560, 1314]);
    assert_eq!(c.status(), Status::Waiting(Refusal::GuardLost));
    let hides = w.count("hide:");
    w.now += 1_000;
    c.tick(&mut w);
    assert_eq!(w.count("hide:"), hides, "nothing is hidden without a guard, so nothing is taken");
    assert_eq!(w.count("dock.take"), 0);
}

#[test]
fn a_restore_run_twice_does_not_touch_the_dock_or_the_work_area_again() {
    let (mut c, mut w) = settled();
    c.set_wanted(&mut w, false);
    let n = w.calls.len();
    c.quit(&mut w);
    c.tick(&mut w);
    assert_eq!(w.calls.len(), n, "{:?}", &w.calls[n..]);
}

// ---------------------------------------------------------------- while it holds

#[test]
fn a_settled_edge_is_quiet_on_every_tick() {
    let (mut c, mut w) = settled();
    for _ in 0..10 {
        w.now += 1_000;
        c.tick(&mut w);
    }
    assert_eq!((w.count("work:"), w.count("dock."), w.count("save"), w.count("hide:")), (0, 0, 0, 0), "{:?}", w.calls);
}

#[test]
fn explorer_taking_the_strip_back_is_answered_a_few_times_and_then_left_alone() {
    let (mut c, mut w) = world(true);
    on(&mut c, &mut w);
    let first = w.work_sets.len();
    assert_eq!(first, 2, "both monitors, in one go");
    for _ in 0..8 {
        // Explorer recomputes: the strip is back
        w.monitors[0].work = [0, 0, 2560, 1392];
        w.now += 1_000;
        c.work_area_changed(&mut w);
    }
    // the first batch used one slot of four; three more were allowed
    assert_eq!(w.work_sets.len(), first + 3, "{:?}", w.work_sets);
    assert!(w.lines.iter().any(|l| l.contains("too often")));
    // and a minute later it is answered again
    w.now += 60_000;
    c.work_area_changed(&mut w);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

#[test]
fn another_toolbar_on_the_primary_monitor_leaves_that_monitor_and_the_dock_alone() {
    let (mut c, mut w) = world(true);
    w.monitors[0].work = [0, 30, 2560, 1314]; // a third-party bar at the top as well
    on(&mut c, &mut w);
    assert_eq!(w.dock_edge, None, "the dock keeps its own registration here");
    assert_eq!(work_a(&w), [0, 30, 2560, 1314]);
    assert_eq!(work_b(&w), B, "the other monitor is still ours");
    assert_eq!(w.state.original_work_areas(), vec![WorkAreaRec { monitor: "B".into(), rect: [-1440, -200, 0, 2311] }]);
    assert!(w.lines.iter().any(|l| l.contains("another toolbar")));
    c.quit(&mut w);
    assert_eq!(work_b(&w), [-1440, -200, 0, 2311]);
    assert_eq!(w.count("dock.give"), 0, "the dock was never told, so there is nothing to give back");
}

#[test]
fn the_docks_band_changing_moves_the_edge_and_keeps_the_first_originals() {
    let (mut c, mut w) = settled();
    c.set_band(&mut w, Some(92));
    assert_eq!(w.dock_edge, Some([0, 1348, 2560, 1440]));
    assert_eq!(work_a(&w), [0, 0, 2560, 1348]);
    assert_eq!(w.state.original_work_areas()[0].rect, [0, 0, 2560, 1392], "the user's own rectangle, not ours");
    // dock_reserve switched off
    w.now += 1_000;
    c.set_band(&mut w, None);
    assert_eq!(w.dock_edge, Some([0, 1440, 2560, 1440]));
    assert_eq!(work_a(&w), A);
    // the same wish again is no change at all
    let n = w.calls.len();
    c.set_band(&mut w, None);
    assert_eq!(w.calls.len(), n);
}

#[test]
fn the_docks_own_band_changes_are_not_held_back_by_the_throttle_that_guards_against_explorer() {
    // The thin dock changes its band when a window is maximised and restored (92 <-> 68 px, never faster than every 2 s). The work
    // area follows every one of them, however many there are in a minute: the throttle is for Explorer taking the work area back, not
    // for the dock's own word.
    let (mut c, mut w) = settled();
    for i in 0..12u32 {
        w.now += 2_000;
        let band = if i % 2 == 0 { 92 } else { 68 };
        c.set_band(&mut w, Some(band));
        assert_eq!(work_a(&w), [0, 0, 2560, 1440 - band], "flip {i} at {} ms", w.now);
        assert_eq!(w.dock_edge, Some([0, 1440 - band, 2560, 1440]), "flip {i}");
    }
    assert!(!w.lines.iter().any(|l| l.contains("too often")), "{:?}", w.lines);
}

#[test]
fn explorer_taking_the_work_area_back_is_still_throttled_after_the_docks_own_changes() {
    let (mut c, mut w) = settled();
    for (i, band) in [92, 68, 92, 68, 92, 68].into_iter().enumerate() {
        w.now += 2_000;
        c.set_band(&mut w, Some(band));
        assert_eq!(work_a(&w)[3], 1440 - band, "{i}");
    }
    // now Explorer fights: the answers are limited as ever (the dock's own sets did not use up the slots)
    let before = w.work_sets.len();
    for _ in 0..8 {
        w.monitors[0].work = [0, 0, 2560, 1392];
        w.now += 1_000;
        c.work_area_changed(&mut w);
    }
    assert_eq!(w.work_sets.len(), before + 3, "{:?}", w.work_sets);
    assert!(w.lines.iter().any(|l| l.contains("too often")));
}

#[test]
fn a_band_that_changes_before_the_hide_is_just_the_wish_when_it_comes() {
    let (mut c, mut w) = world(true);
    c.set_band(&mut w, Some(92));
    assert_eq!(w.calls.len(), 0, "nothing is wanted yet: nothing is touched");
    on(&mut c, &mut w);
    assert_eq!(work_a(&w), [0, 0, 2560, 1348]);
}

// ---------------------------------------------------------------- Explorer restarting, and a broadcast that is not a restart

#[test]
fn a_taskbar_created_that_is_not_an_explorer_restart_keeps_the_hide_accounted_for() {
    // our own tray host broadcasts TaskbarCreated when it starts and stops: the windows are the same
    let (mut c, mut w) = settled();
    c.explorer_restarted(&mut w);
    assert_eq!(c.status(), Status::Hidden(2), "the hidden windows are still ours");
    assert!(c.holds_windows());
    assert_eq!((w.count("hide:"), w.count("dock."), w.count("work:"), w.count("save")), (0, 0, 0, 0), "{:?}", w.calls);
}

#[test]
fn a_real_explorer_restart_hides_the_new_taskbar_and_sets_the_work_area_again_without_a_new_dock_round() {
    let (mut c, mut w) = settled();
    // the new Explorer: new windows of a new process, visible, and a work area computed from them alone
    {
        let mut d = w.desktop.borrow_mut();
        for x in d.iter_mut() {
            x.handle += 0x1000;
            x.pid = 9900;
            x.start = EXPLORER_START + 77;
            x.visible = true;
        }
    }
    w.monitors[0].work = [0, 0, 2560, 1392];
    w.monitors[1].work = [-1440, -200, 0, 2311];
    c.explorer_restarted(&mut w);
    assert_eq!(c.status(), Status::Hidden(2));
    assert_eq!(visible_handles(&w.desktop), Vec::<isize>::new());
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
    assert_eq!(w.count("dock.take"), 0, "the dock already has the edge");
    assert_eq!(w.count("dock.give"), 0);
}

// ---------------------------------------------------------------- nothing for a user who did not ask

#[test]
fn a_controller_that_is_never_asked_does_nothing_at_all() {
    let (mut c, mut w) = world(true);
    c.tick(&mut w);
    c.work_area_changed(&mut w);
    c.displays_changed(&mut w);
    c.set_band(&mut w, Some(10));
    assert!(w.calls.is_empty(), "{:?}", w.calls);
}

#[test]
fn an_earlier_unfinished_run_s_rectangle_is_kept_not_overwritten() {
    let (mut c, mut w) = world(true);
    // an earlier run changed the work area and was killed; the file still holds the user's real rectangle
    w.state.set_original_work_areas(&[WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1380] }]);
    w.monitors[0].work = [0, 0, 2560, 1362];
    w.dock_band = None;
    on(&mut c, &mut w);
    assert_eq!(w.state.original_work_areas()[0].rect, [0, 0, 2560, 1380]);
}

// ---------------------------------------------------------------- what the review found

/// A desktop with only the primary monitor's taskbar (the second monitor is not plugged in yet).
fn primary_only() -> (Controller, FakeWorld) {
    let c = Controller::new(HOST_PID, HOST_START).with_band(Some(BAND));
    let mut w = FakeWorld::new(Rc::new(RefCell::new(vec![window(0x1001, PRIMARY, [0, 1392, 2560, 1440])])));
    w.monitors = vec![MonitorGeom { id: "A".into(), rect: A, work: [0, 0, 2560, 1392 - BAND], primary: true }];
    w.dock_band = Some(BAND);
    w.dock_wants_band = Some(BAND);
    (c, w)
}

/// The second monitor is plugged in: its taskbar exists (visible) and Explorer computed its work area from it.
fn plug_in_b(w: &mut FakeWorld) {
    w.desktop.borrow_mut().push(window(0x1002, SECONDARY, [-1440, 2311, 0, 2359]));
    w.monitors.push(MonitorGeom { id: "B".into(), rect: B, work: [-1440, -200, 0, 2311], primary: false });
}

#[test]
fn a_monitor_that_becomes_ours_after_the_edge_was_taken_is_written_down_before_its_work_area_is_set() {
    let (mut c, mut w) = primary_only();
    on(&mut c, &mut w);
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(w.state.original_work_areas(), vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1392] }]);
    w.calls.clear();
    // the user plugs a second monitor in; its taskbar is hidden by the next pass and its work area is set
    plug_in_b(&mut w);
    w.now += 1_000;
    c.displays_changed(&mut w);
    assert_eq!(c.status(), Status::Hidden(2));
    assert_eq!(work_b(&w), B);
    assert_eq!(
        w.state.original_work_areas(),
        vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 2560, 1392] }, WorkAreaRec { monitor: "B".into(), rect: [-1440, -200, 0, 2311] }],
        "the first record is kept and the new monitor's is added"
    );
    let saved = w.calls.iter().rposition(|c| c == "save").unwrap();
    assert!(saved < w.pos("work:-1440").unwrap(), "written down before the first change to it: {:?}", w.calls);
    // and the way out puts it back
    c.quit(&mut w);
    assert_eq!(work_b(&w), [-1440, -200, 0, 2311], "the shown taskbar does not overlap the second monitor's windows");
    assert_eq!(work_a(&w), [0, 0, 2560, 1314]);
}

#[test]
fn a_new_monitor_whose_original_cannot_be_written_is_not_touched() {
    let (mut c, mut w) = primary_only();
    on(&mut c, &mut w);
    plug_in_b(&mut w);
    w.save_fails_after = Some(w.calls.iter().filter(|c| *c == "save").count() + 1); // the record of the new window is written, the originals are not
    w.now += 1_000;
    c.displays_changed(&mut w);
    assert_eq!(work_b(&w), [-1440, -200, 0, 2311], "no change without the way back on disk");
    assert_eq!(w.state.original_work_areas().len(), 1);
}

#[test]
fn a_resolution_change_while_hidden_is_not_answered_with_a_rectangle_of_the_old_screen() {
    let (mut c, mut w) = settled();
    // 2560x1440 became 1920x1080: Explorer moved the taskbar and recomputed
    w.desktop.borrow_mut()[0].rect = [0, 1032, 1920, 1080];
    w.monitors[0].rect = [0, 0, 1920, 1080];
    w.monitors[0].work = [0, 0, 1920, 1080];
    c.quit(&mut w);
    assert_eq!(w.count("work:0,0,2560,1392"), 0, "the old screen's rectangle is not applied: {:?}", w.calls);
    assert!(w.work_sets.contains(&[0, 0, 1920, 1032]), "worked out again: the monitor less the taskbar: {:?}", w.work_sets);
    assert_eq!(work_b(&w), [-1440, -200, 0, 2311], "the other monitor did not change");
    assert!(w.lines.iter().any(|l| l.contains("screen changed")));
    assert!(w.state.hidden_taskbar().is_none() && w.state.original_work_areas().is_empty(), "and the restore is complete");
}

#[test]
fn a_monitor_that_was_unplugged_while_hidden_gets_no_work_area_and_its_rectangle_lands_nowhere_else() {
    let (mut c, mut w) = settled();
    w.monitors.remove(1);
    w.desktop.borrow_mut().remove(1);
    c.quit(&mut w);
    assert_eq!(w.count("work:-1440"), 0, "{:?}", w.calls);
    assert_eq!(w.work_sets, vec![[0, 0, 2560, 1362], [-1440, -200, 0, 2359], [0, 0, 2560, 1392]], "only the primary monitor is put back");
    assert_eq!((c.edge(), c.status()), (Edge::Free, Status::Off));
    assert!(w.state.original_work_areas().is_empty() && w.guard_done);
}

#[test]
fn a_taskbar_that_cannot_be_hidden_after_an_explorer_restart_gives_the_dock_its_band_back() {
    let (mut c, mut w) = settled();
    w.calls.clear();
    // the new Explorer: new, visible windows, and a work area computed from them (the strip, and our band is not in it)
    {
        let mut d = w.desktop.borrow_mut();
        for x in d.iter_mut() {
            x.handle += 0x1000;
            x.pid = 9900;
            x.start = EXPLORER_START + 77;
            x.visible = true;
        }
    }
    w.hide_lost = vec![0x2001, 0x2002]; // Explorer is busy and drops the queued hide
    w.monitors[0].work = [0, 0, 2560, 1392];
    w.monitors[1].work = [-1440, -200, 0, 2311];
    c.explorer_restarted(&mut w);
    assert!(!matches!(c.status(), Status::Hidden(_)));
    assert_eq!((c.edge(), w.dock_edge, w.count("dock.give")), (Edge::Free, None, 1), "the dock is not left on the edge with no band and a visible taskbar: {:?}", w.calls);
    assert_eq!(w.dock_band, Some(BAND), "and registers its band with Explorer again");
    // later the hide goes through: the edge is taken again
    w.hide_lost.clear();
    w.now += 1_000;
    c.tick(&mut w);
    assert_eq!((c.status(), c.edge()), (Status::Hidden(2), Edge::Ours));
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

#[test]
fn the_same_while_the_dock_is_still_letting_go() {
    let (mut c, mut w) = world(true);
    w.dock_release_after = 50;
    on(&mut c, &mut w);
    assert!(matches!(c.edge(), Edge::Releasing { .. }));
    {
        let mut d = w.desktop.borrow_mut();
        for x in d.iter_mut() {
            x.handle += 0x1000;
            x.pid = 9900;
            x.start = EXPLORER_START + 77;
            x.visible = true;
        }
    }
    w.hide_lost = vec![0x2001, 0x2002];
    c.explorer_restarted(&mut w);
    assert_eq!((c.edge(), w.dock_edge, c.edge_waiting()), (Edge::Free, None, false), "the 100 ms look does not run for ever for nothing");
    assert_eq!(w.count("work:"), 0);
}

#[test]
fn a_removal_that_explorer_did_not_take_sets_nothing_and_the_dock_gets_its_band_back() {
    // the dock says it let go, but its message did not get through (the tray host's window is the first Shell_TrayWnd): the work area
    // still has the band, and a work area set on top of it would be a second strip
    let (mut c, mut w) = world(true);
    w.explorer_keeps_band = true;
    on(&mut c, &mut w);
    assert!(matches!(c.edge(), Edge::Releasing { .. }), "the dock's own word is not enough");
    assert_eq!(w.count("work:"), 0, "{:?}", w.calls);
    assert_eq!(work_a(&w), [0, 0, 2560, 1314]);
    w.now += EDGE_DOCK_WAIT_MS;
    c.tick(&mut w);
    assert_eq!((c.edge(), w.count("work:"), w.count("dock.give")), (Edge::Free, 0, 1));
    assert_eq!(work_a(&w), [0, 0, 2560, 1314], "nothing changed for the user");
    // the route works at the next try
    w.explorer_keeps_band = false;
    w.now += EDGE_RETRY_MS[0];
    c.tick(&mut w);
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(work_a(&w), [0, 0, 2560, 1362]);
}

#[test]
fn nothing_is_taken_until_the_dock_has_said_what_its_band_is() {
    // the hider starts before the dock has said anything (no band known): the taskbar is hidden, the edge waits
    let mut c = Controller::new(HOST_PID, HOST_START);
    let (_, mut w) = world(true);
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(2));
    assert_eq!((w.count("dock."), w.count("work:"), c.edge()), (0, 0, Edge::Free), "{:?}", w.calls);
    c.tick(&mut w);
    assert_eq!(w.count("work:"), 0, "still waiting on a tick");
    // the dock speaks: the first set is the right one (no "whole monitor" first and a correction after)
    c.set_band(&mut w, Some(BAND));
    assert_eq!(c.edge(), Edge::Ours);
    assert_eq!(w.work_sets, vec![[0, 0, 2560, 1362], B]);
    assert_eq!(w.dock_edge, Some([0, 1362, 2560, 1440]));
    // "no band" is also a word
    let mut c = Controller::new(HOST_PID, HOST_START);
    let (_, mut w) = world(false);
    on(&mut c, &mut w);
    assert_eq!(w.count("work:"), 0);
    c.set_band(&mut w, None);
    assert_eq!((c.edge(), work_a(&w)), (Edge::Ours, A));
    assert_eq!(w.dock_edge, Some([0, 1440, 2560, 1440]));
}

#[test]
fn a_taskbar_on_the_left_is_hidden_but_the_edge_and_the_work_areas_are_left_alone() {
    let mut c = Controller::new(HOST_PID, HOST_START).with_band(Some(BAND));
    let mut w = FakeWorld::new(Rc::new(RefCell::new(vec![window(0x1001, PRIMARY, [0, 0, 48, 1440])])));
    w.monitors = vec![MonitorGeom { id: "A".into(), rect: A, work: [48, 0, 2560, 1440 - BAND], primary: true }];
    w.dock_band = Some(BAND);
    w.dock_wants_band = Some(BAND);
    on(&mut c, &mut w);
    assert_eq!(c.status(), Status::Hidden(1));
    assert_eq!((w.count("dock."), w.count("work:"), c.edge(), w.dock_edge), (0, 0, Edge::Free, None), "{:?}", w.calls);
    assert_eq!(w.dock_band, Some(BAND), "the dock keeps its own registration");
    assert!(w.state.original_work_areas().is_empty());
}

#[test]
fn an_auto_hiding_taskbar_that_is_up_is_put_back_as_the_whole_monitor() {
    // the work area is the whole monitor although the taskbar sits on the edge: the user has nothing reserved
    let (mut c, mut w) = world(false);
    w.monitors[0].work = A;
    w.dock_band = None;
    on(&mut c, &mut w);
    assert_eq!(w.state.original_work_areas()[0], WorkAreaRec { monitor: "A".into(), rect: A }, "what was read, not the monitor less a strip nobody reserved");
    c.quit(&mut w);
    assert_eq!(w.work_sets.iter().filter(|r| **r == A).count(), 1, "put back as the whole monitor: {:?}", w.work_sets);
}
