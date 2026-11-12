//! Table-driven tests of the pure state machine: every transition of plan 6.2, the edges of the
//! crash window, a clock that goes backwards, and the order of the way back to Explorer.

use crate::machine::*;

const S: Millis = 1_000;

// ---- helpers -------------------------------------------------------------------------------

fn facts() -> Facts {
    Facts::default()
}

fn kinds(actions: &[Action]) -> Vec<ActionKind> {
    actions.iter().map(Action::kind).collect()
}

fn boot(f: Facts, now: Millis) -> (State, Vec<Action>) {
    step(State::new(), Event::Boot(f), now)
}

fn running_state(pid: u32, since: Millis, crashes: Vec<Crash>) -> State {
    State { phase: Phase::Running { pid, since, hung_ticks: 0 }, crashes, last_seen: since, build: Some("26300.1".into()) }
}

fn with_phase(phase: Phase) -> State {
    // In a real run `last_seen` is never behind a timer's start.
    let last_seen = match &phase {
        Phase::Starting { since, .. } | Phase::Running { since, .. } | Phase::Backoff { since, .. } => *since,
        _ => 0,
    };
    State { phase, crashes: vec![], last_seen, build: None }
}

fn crash(at: Millis, uptime_ms: Millis) -> Crash {
    Crash { at, uptime_ms }
}

/// Boots, starts a host and makes it ready; returns the running state.
fn up(now: Millis) -> State {
    let (st, _) = boot(facts(), now);
    let (st, _) = step(st, Event::HostStarted { pid: 7 }, now);
    let (st, _) = step(st, Event::HostReady, now);
    assert!(matches!(st.phase, Phase::Running { pid: 7, .. }));
    st
}

/// Answers every fall-back action in turn with `answer`, until the machine exits. Returns the actions it issued.
fn walk(mut st: State, first: Vec<Action>, now: Millis, answer: &dyn Fn(&Action) -> bool) -> (State, Vec<Action>) {
    let mut seen = Vec::new();
    let mut pending = first;
    for _ in 0..40 {
        let Some(a) = pending.into_iter().next() else { panic!("the machine issued no action; seen {seen:?}") };
        seen.push(a.clone());
        if matches!(a, Action::Exit { .. }) {
            return (st, seen);
        }
        let (n, acts) = step(st, Event::Done { kind: a.kind(), ok: answer(&a) }, now);
        st = n;
        pending = acts;
    }
    panic!("the fall-back did not finish: {seen:?}");
}

fn fallback_reason(st: &State) -> Option<Reason> {
    match &st.phase {
        Phase::Fallback(fb) => Some(fb.reason()),
        Phase::Done(Outcome::FellBack(r)) => Some(*r),
        _ => None,
    }
}

fn all_ok(_: &Action) -> bool {
    true
}

fn only_probe_fails(a: &Action) -> bool {
    a.kind() != ActionKind::ProbeExplorer
}

// ---- boot ----------------------------------------------------------------------------------

#[test]
fn a_normal_boot_claims_the_identity_and_starts_the_host() {
    let (st, a) = boot(facts(), 1000);
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
    assert_eq!(st.phase, Phase::Launching);
}

#[test]
fn a_changed_build_runs_the_self_test_first() {
    let (st, a) = boot(Facts { build_changed: true, ..facts() }, 0);
    assert_eq!(a, vec![Action::SelfTest]);
    assert_eq!(st.phase, Phase::SelfTest);
    let (st2, a) = step(st.clone(), Event::Done { kind: ActionKind::SelfTest, ok: true }, 0);
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
    assert_eq!(st2.phase, Phase::Launching);
    let (st3, a) = step(st, Event::Done { kind: ActionKind::SelfTest, ok: false }, 0);
    assert_eq!(fallback_reason(&st3), Some(Reason::SelfTestFailed));
    assert!(matches!(a[0], Action::WriteState(Persist::FallbackStart { reason: Reason::SelfTestFailed, .. })));
}

#[test]
fn a_pending_restore_is_undone_before_anything_else() {
    let (_, a) = boot(Facts { pending_restore: true, ..facts() }, 0);
    assert_eq!(kinds(&a), vec![ActionKind::RestoreWorkAreas, ActionKind::ShowTaskbar, ActionKind::ClaimIdentity, ActionKind::StartHost]);
    let (_, a) = boot(Facts { pending_restore: true, build_changed: true, ..facts() }, 0);
    assert_eq!(kinds(&a), vec![ActionKind::RestoreWorkAreas, ActionKind::ShowTaskbar, ActionKind::SelfTest]);
}

#[test]
fn safe_mode_and_shift_start_explorer_but_leave_the_value() {
    for (f, reason) in [(Facts { safe_mode: true, ..facts() }, Reason::SafeMode), (Facts { shift_held: true, ..facts() }, Reason::UserSkip)] {
        let (st, a) = boot(f, 0);
        assert_eq!(fallback_reason(&st), Some(reason));
        let (end, seen) = walk(st, a, 0, &only_probe_fails);
        assert!(!kinds(&seen).contains(&ActionKind::ClearShellValue), "{reason:?} must not delete the value");
        assert!(kinds(&seen).contains(&ActionKind::StartExplorer));
        assert_eq!(seen.last(), Some(&Action::Exit { code: 0 }));
        assert_eq!(end.outcome(), Some(Outcome::FellBack(reason)));
    }
}

#[test]
fn safe_mode_beats_shift_and_panic_beats_both() {
    let (st, _) = boot(Facts { safe_mode: true, shift_held: true, ..facts() }, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::SafeMode));
    let (st, _) = boot(Facts { safe_mode: true, shift_held: true, panic: true, ..facts() }, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::Panic));
}

#[test]
fn panic_hands_over_to_a_running_guard_and_starts_nothing() {
    let (st, a) = boot(Facts { panic: true, panic_delegated: true, ..facts() }, 0);
    assert_eq!(a, vec![Action::Exit { code: 0 }]);
    assert_eq!(st.outcome(), Some(Outcome::Delegated));
}

#[test]
fn an_unresolved_earlier_fallback_is_finished_first() {
    let (st, a) = boot(Facts { unresolved: Some(Reason::CrashLoop), ..facts() }, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
    assert!(matches!(a[0], Action::WriteState(Persist::FallbackStart { reason: Reason::CrashLoop, .. })));
    // A reason that leaves the value alone cannot be unresolved.
    let (st, _) = boot(Facts { unresolved: Some(Reason::SafeMode), ..facts() }, 0);
    assert_eq!(st.phase, Phase::Launching);
}

#[test]
fn a_crash_loop_in_the_persisted_log_goes_straight_to_explorer() {
    let now = 10_000_000;
    let crashes = vec![crash(now - 50 * S, 20 * S), crash(now - 30 * S, 20 * S), crash(now - 5 * S, 20 * S)];
    let (st, _) = boot(Facts { crashes, ..facts() }, now);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
}

// ---- the fall-back sequence ----------------------------------------------------------------

#[test]
fn the_way_back_has_the_planned_order_and_clears_the_value_before_explorer_starts() {
    let st = up(0);
    let (st, a) = step(st, Event::EscapePressed, 5 * S);
    let (end, seen) = walk(st, a, 5 * S, &only_probe_fails);
    let k = kinds(&seen);
    assert_eq!(
        k,
        vec![
            ActionKind::WriteState,
            ActionKind::ReleaseIdentity,
            ActionKind::ClearShellValue,
            ActionKind::KillHost,
            ActionKind::RestoreWorkAreas,
            ActionKind::ShowTaskbar,
            ActionKind::ProbeExplorer,
            ActionKind::StartExplorer,
            ActionKind::WaitExplorerUp,
            ActionKind::WriteState,
            ActionKind::Exit,
        ]
    );
    let pos = |kind| k.iter().position(|x| *x == kind).unwrap();
    assert!(pos(ActionKind::ReleaseIdentity) < pos(ActionKind::ClearShellValue), "identity first");
    assert!(pos(ActionKind::ClearShellValue) < pos(ActionKind::StartExplorer), "the value goes before Explorer starts");
    assert!(pos(ActionKind::ReleaseIdentity) < pos(ActionKind::StartExplorer));
    assert!(matches!(seen[0], Action::WriteState(Persist::FallbackStart { reason: Reason::Escape, .. })));
    assert!(matches!(seen[9], Action::WriteState(Persist::FallbackEnd { reason: Reason::Escape, cleared: true, explorer_up: true })));
    assert_eq!(seen[10], Action::Exit { code: 0 });
    assert_eq!(end.outcome(), Some(Outcome::FellBack(Reason::Escape)));
}

#[test]
fn explorer_already_up_is_not_started_a_second_time() {
    let (st, a) = boot(Facts { panic: true, ..facts() }, 0);
    let (_, seen) = walk(st, a, 0, &all_ok);
    let k = kinds(&seen);
    assert!(k.contains(&ActionKind::ProbeExplorer));
    assert!(!k.contains(&ActionKind::StartExplorer) && !k.contains(&ActionKind::WaitExplorerUp));
    assert!(matches!(seen[seen.len() - 2], Action::WriteState(Persist::FallbackEnd { explorer_up: true, cleared: true, .. })));
}

#[test]
fn when_clearing_the_value_fails_explorer_still_starts_and_the_user_is_told() {
    let st = up(0);
    let (st, a) = step(st, Event::EscapePressed, 0);
    let (_, seen) = walk(st, a, 0, &|a| a.kind() != ActionKind::ClearShellValue && a.kind() != ActionKind::ProbeExplorer);
    let k = kinds(&seen);
    assert!(k.contains(&ActionKind::StartExplorer), "the user must get a desktop even if the value is stuck");
    assert!(matches!(seen[seen.len() - 3], Action::WriteState(Persist::FallbackEnd { cleared: false, explorer_up: true, .. })));
    assert_eq!(seen[seen.len() - 2], Action::ShowMessage(Message { value_left: true, explorer_down: false }));
    assert_eq!(seen[seen.len() - 1], Action::Exit { code: 0 });
}

#[test]
fn explorer_that_never_comes_up_is_tried_twice_then_reported() {
    let (st, a) = boot(Facts { panic: true, ..facts() }, 0);
    // The wait never succeeds.
    let (end, seen) = walk(st, a, 0, &|a| !matches!(a.kind(), ActionKind::WaitExplorerUp | ActionKind::ProbeExplorer));
    let k = kinds(&seen);
    assert_eq!(k.iter().filter(|x| **x == ActionKind::StartExplorer).count(), 2);
    assert_eq!(k.iter().filter(|x| **x == ActionKind::WaitExplorerUp).count(), 2);
    assert_eq!(seen[seen.len() - 2], Action::ShowMessage(Message { value_left: false, explorer_down: true }));
    assert_eq!(seen[seen.len() - 1], Action::Exit { code: 1 });
    assert_eq!(end.outcome(), Some(Outcome::FellBack(Reason::Panic)));
}

#[test]
fn a_start_that_fails_outright_counts_as_an_attempt() {
    let (st, a) = boot(Facts { panic: true, ..facts() }, 0);
    let (_, seen) = walk(st, a, 0, &|a| !matches!(a.kind(), ActionKind::StartExplorer | ActionKind::ProbeExplorer));
    let k = kinds(&seen);
    assert_eq!(k.iter().filter(|x| **x == ActionKind::StartExplorer).count(), 2);
    assert!(!k.contains(&ActionKind::WaitExplorerUp));
    assert_eq!(seen[seen.len() - 1], Action::Exit { code: 1 });
}

#[test]
fn explorer_that_needs_a_second_try_is_fine() {
    let (st, a) = boot(Facts { panic: true, ..facts() }, 0);
    let waits = std::cell::Cell::new(0);
    let (_, seen) = walk(st, a, 0, &|a| match a.kind() {
        ActionKind::ProbeExplorer => false,
        ActionKind::WaitExplorerUp => {
            waits.set(waits.get() + 1);
            waits.get() == 2
        }
        _ => true,
    });
    assert_eq!(seen[seen.len() - 1], Action::Exit { code: 0 });
    assert!(!seen.iter().any(|a| matches!(a, Action::ShowMessage(_))));
}

#[test]
fn both_failures_give_one_message_that_says_both() {
    let (st, a) = boot(Facts { panic: true, ..facts() }, 0);
    let (_, seen) = walk(st, a, 0, &|a| !matches!(a.kind(), ActionKind::ClearShellValue | ActionKind::WaitExplorerUp | ActionKind::ProbeExplorer));
    assert_eq!(seen[seen.len() - 2], Action::ShowMessage(Message { value_left: true, explorer_down: true }));
}

#[test]
fn safe_mode_with_a_dead_explorer_reports_only_the_explorer() {
    let (st, a) = boot(Facts { safe_mode: true, ..facts() }, 0);
    let (_, seen) = walk(st, a, 0, &|a| !matches!(a.kind(), ActionKind::WaitExplorerUp | ActionKind::ProbeExplorer));
    assert_eq!(seen[seen.len() - 2], Action::ShowMessage(Message { value_left: false, explorer_down: true }));
}

#[test]
fn stale_results_do_not_move_the_fallback() {
    let st = up(0);
    let (st, _) = step(st, Event::EscapePressed, 0);
    let before = st.clone();
    // The first step wants the answer of WriteState; a late "taskbar shown" from an earlier phase is ignored.
    let (after, a) = step(st, Event::Done { kind: ActionKind::ShowTaskbar, ok: true }, 0);
    assert_eq!(after, before);
    assert!(a.is_empty());
}

#[test]
fn a_second_trigger_while_falling_back_changes_nothing() {
    let st = up(0);
    let (st, _) = step(st, Event::EscapePressed, 0);
    let before = st.clone();
    for ev in [Event::EscapePressed, Event::HostReady, Event::HostExited { pid: 7, code: 1 }, Event::Tick { host_hung: true }, Event::HostSpawnFailed] {
        let (after, a) = step(st.clone(), ev, 0);
        assert_eq!(after, before);
        assert!(a.is_empty());
    }
}

#[test]
fn panic_upgrades_a_safe_mode_fallback_that_has_not_reached_the_value() {
    let (st, _) = boot(Facts { safe_mode: true, ..facts() }, 0);
    let (st, a) = step(st, Event::PanicRequested, 0);
    assert!(a.is_empty());
    assert_eq!(fallback_reason(&st), Some(Reason::Panic));
    // Once the sequence is past the value, a late panic changes nothing.
    let (st, a) = boot(Facts { safe_mode: true, ..facts() }, 0);
    let (st, _a2) = step(st, Event::Done { kind: ActionKind::WriteState, ok: true }, 0);
    let (st, _a3) = step(st, Event::Done { kind: ActionKind::ReleaseIdentity, ok: true }, 0);
    let (st, _a4) = step(st, Event::Done { kind: ActionKind::KillHost, ok: true }, 0);
    assert!(matches!(a[0], Action::WriteState(_)));
    let (st, _) = step(st, Event::PanicRequested, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::SafeMode));
}

// ---- from any state ------------------------------------------------------------------------

fn every_live_phase() -> Vec<State> {
    vec![
        State::new(),
        with_phase(Phase::SelfTest),
        with_phase(Phase::Launching),
        with_phase(Phase::Starting { pid: 3, since: 0 }),
        running_state(3, 0, vec![]),
        with_phase(Phase::Backoff { since: 0, delay: 1000 }),
    ]
}

#[test]
fn panic_works_from_every_state() {
    for st in every_live_phase() {
        let label = format!("{:?}", st.phase);
        let (n, a) = step(st, Event::PanicRequested, 50);
        assert_eq!(fallback_reason(&n), Some(Reason::Panic), "{label}");
        assert!(matches!(a[0], Action::WriteState(Persist::FallbackStart { reason: Reason::Panic, .. })), "{label}");
        let (end, seen) = walk(n, a, 50, &only_probe_fails);
        assert!(kinds(&seen).contains(&ActionKind::ClearShellValue), "{label}");
        assert_eq!(end.outcome(), Some(Outcome::FellBack(Reason::Panic)), "{label}");
    }
}

#[test]
fn escape_works_from_every_live_state() {
    for st in every_live_phase() {
        let label = format!("{:?}", st.phase);
        let (n, _) = step(st, Event::EscapePressed, 50);
        assert_eq!(fallback_reason(&n), Some(Reason::Escape), "{label}");
    }
}

#[test]
fn the_session_ending_just_exits_from_every_live_state() {
    for st in every_live_phase() {
        let (n, a) = step(st, Event::SessionEnding, 50);
        assert_eq!(a, vec![Action::Exit { code: 0 }]);
        assert_eq!(n.outcome(), Some(Outcome::SessionEnded));
    }
}

#[test]
fn a_finished_machine_ignores_everything() {
    let st = with_phase(Phase::Done(Outcome::SessionEnded));
    for ev in [Event::PanicRequested, Event::EscapePressed, Event::SessionEnding, Event::HostReady, Event::Tick { host_hung: true }] {
        let (n, a) = step(st.clone(), ev, 0);
        assert_eq!(n, st);
        assert!(a.is_empty());
    }
}

// ---- starting, running, hung ---------------------------------------------------------------

#[test]
fn ready_moves_to_running_and_signals_the_shell_ready_event() {
    let (st, _) = boot(facts(), 0);
    let (st, a) = step(st, Event::HostStarted { pid: 9 }, 100);
    assert_eq!(st.phase, Phase::Starting { pid: 9, since: 100 });
    assert!(a.is_empty());
    let (st, a) = step(st, Event::HostReady, 200);
    assert!(matches!(st.phase, Phase::Running { pid: 9, since: 100, .. }));
    assert_eq!(kinds(&a), vec![ActionKind::SignalShellReady, ActionKind::WriteState]);
    assert!(matches!(&a[1], Action::WriteState(Persist::Ready { .. })));
}

#[test]
fn a_host_that_dies_before_ready_has_crashed_whatever_its_exit_code() {
    for code in [0, 1, EXIT_RESTART, -1073741819] {
        let (st, _) = boot(facts(), 0);
        let (st, _) = step(st, Event::HostStarted { pid: 9 }, 0);
        let (st, a) = step(st, Event::HostExited { pid: 9, code }, 500);
        assert_eq!(st.crashes.len(), 1, "code {code}");
        assert!(kinds(&a).contains(&ActionKind::RestoreWorkAreas));
        assert_eq!(st.crashes[0].uptime_ms, 500);
    }
}

#[test]
fn the_ready_deadline_edges() {
    let (st, _) = boot(facts(), 0);
    let (st, _) = step(st, Event::HostStarted { pid: 9 }, 1000);
    let (same, a) = step(st.clone(), Event::Tick { host_hung: false }, 1000 + READY_DEADLINE_MS - 1);
    assert!(a.is_empty() && matches!(same.phase, Phase::Starting { .. }));
    let (dead, a) = step(st.clone(), Event::Tick { host_hung: false }, 1000 + READY_DEADLINE_MS);
    assert_eq!(a[0], Action::KillHost);
    assert_eq!(dead.crashes.len(), 1);
    assert_eq!(dead.crashes[0].uptime_ms, READY_DEADLINE_MS);
    assert_eq!(st.next_wake_ms(1000), Some(READY_DEADLINE_MS));
    assert_eq!(st.next_wake_ms(1000 + READY_DEADLINE_MS - 1), Some(1));
    assert_eq!(st.next_wake_ms(1000 + READY_DEADLINE_MS + 5), Some(1));
}

#[test]
fn a_spawn_failure_is_a_crash_with_no_uptime() {
    let (st, _) = boot(facts(), 10);
    let (st, a) = step(st, Event::HostSpawnFailed, 10);
    assert_eq!(st.crashes, vec![crash(10, 0)]);
    assert!(kinds(&a).contains(&ActionKind::ShowTaskbar));
}

#[test]
fn exit_codes_of_a_running_host() {
    // EXIT_LEAVE: the user asked to leave.
    let (st, a) = step(up(0), Event::HostExited { pid: 7, code: EXIT_LEAVE }, 60 * S);
    assert_eq!(fallback_reason(&st), Some(Reason::UserExit));
    assert!(matches!(a[0], Action::WriteState(Persist::FallbackStart { reason: Reason::UserExit, .. })));
    // Restart request after a decent run: relaunch, nothing counted.
    let (st, a) = step(up(0), Event::HostExited { pid: 7, code: EXIT_RESTART }, 60 * S);
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
    assert!(st.crashes.is_empty() && st.phase == Phase::Launching);
    // Exit code 0 after a decent run (a sign-out, a hand-over to another instance) is a plain restart: the switch stays on.
    let (st, a) = step(up(0), Event::HostExited { pid: 7, code: EXIT_CLEAN }, 60 * S);
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
    assert!(st.crashes.is_empty() && st.phase == Phase::Launching);
    // Anything else: a crash.
    for code in [1, 3, -1073741819, 255] {
        let (st, _) = step(up(0), Event::HostExited { pid: 7, code }, 60 * S);
        assert_eq!(st.crashes.len(), 1, "code {code}");
    }
}

#[test]
fn a_restart_request_right_after_start_counts_as_a_crash() {
    for (uptime, counted) in [(FAST_CRASH_MS, true), (FAST_CRASH_MS + 1, false)] {
        let (st, _) = step(up(0), Event::HostExited { pid: 7, code: EXIT_RESTART }, uptime);
        assert_eq!(st.crashes.len() == 1, counted, "uptime {uptime}");
    }
}

#[test]
fn an_exit_of_some_other_process_is_ignored() {
    let (st, a) = step(up(0), Event::HostExited { pid: 999, code: 1 }, 100);
    assert!(a.is_empty() && st.crashes.is_empty());
    let (st, _) = boot(facts(), 0);
    let (st, _) = step(st, Event::HostStarted { pid: 9 }, 0);
    let (st2, a) = step(st.clone(), Event::HostExited { pid: 1, code: 1 }, 5);
    assert!(a.is_empty() && st2.phase == st.phase);
}

#[test]
fn hang_detection_needs_two_checks_in_a_row() {
    let st = up(0);
    let (st, a) = step(st, Event::Tick { host_hung: true }, 10 * S);
    assert!(a.is_empty());
    let (st, a) = step(st, Event::Tick { host_hung: false }, 20 * S);
    assert!(a.is_empty());
    assert!(matches!(st.phase, Phase::Running { hung_ticks: 0, .. }), "a good check resets the count");
    let (st, _) = step(st, Event::Tick { host_hung: true }, 30 * S);
    let (st, a) = step(st, Event::Tick { host_hung: true }, 40 * S);
    assert_eq!(a[0], Action::KillHost);
    assert_eq!(st.crashes.len(), 1);
    assert_eq!(st.crashes[0].uptime_ms, 40 * S);
}

#[test]
fn a_running_host_is_checked_every_ten_seconds() {
    assert_eq!(up(0).next_wake_ms(5), Some(HANG_TICK_MS));
}

// ---- backoff and the crash window -----------------------------------------------------------

#[test]
fn backoff_is_zero_then_one_then_three_seconds() {
    assert_eq!([backoff_for(1), backoff_for(2), backoff_for(3), backoff_for(4), backoff_for(5)], [0, 1000, 3000, 3000, 3000]);
    // First crash: relaunched at once.
    let (st, a) = step(running_state(7, 0, vec![]), Event::HostExited { pid: 7, code: 1 }, 20 * S);
    assert_eq!(kinds(&a), vec![ActionKind::WriteState, ActionKind::RestoreWorkAreas, ActionKind::ShowTaskbar, ActionKind::ClaimIdentity, ActionKind::StartHost]);
    assert_eq!(st.phase, Phase::Launching);
    // Second crash, a minute and a half later (not fast, not within the window): one second.
    let st = running_state(7, 100 * S, vec![crash(20 * S, 20 * S)]);
    let (st, a) = step(st, Event::HostExited { pid: 7, code: 1 }, 130 * S);
    assert_eq!(st.phase, Phase::Backoff { since: 130 * S, delay: 1000 });
    assert_eq!(kinds(&a), vec![ActionKind::WriteState, ActionKind::RestoreWorkAreas, ActionKind::ShowTaskbar]);
    // Third and fourth: three seconds.
    let st = running_state(7, 200 * S, vec![crash(20 * S, 20 * S), crash(130 * S, 20 * S)]);
    let (st, _) = step(st, Event::HostExited { pid: 7, code: 1 }, 230 * S);
    assert_eq!(st.phase, Phase::Backoff { since: 230 * S, delay: 3000 });
}

#[test]
fn backoff_waits_exactly_its_delay() {
    let st = with_phase(Phase::Backoff { since: 1000, delay: 3000 });
    let (same, a) = step(st.clone(), Event::Tick { host_hung: false }, 3999);
    assert!(a.is_empty() && same.phase == st.phase);
    let (n, a) = step(st.clone(), Event::Tick { host_hung: false }, 4000);
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
    assert_eq!(n.phase, Phase::Launching);
    assert_eq!(st.next_wake_ms(1000), Some(3000));
    assert_eq!(st.next_wake_ms(3500), Some(500));
    assert_eq!(st.next_wake_ms(9999), Some(1));
}

#[test]
fn three_crashes_within_a_minute_edges() {
    // Two earlier crashes at 0 s and 30 s, neither fast; the third one decides.
    let earlier = vec![crash(0, 10 * S), crash(30 * S, 10 * S)];
    // Third exactly 60 s after the first: inside the window.
    let (st, _) = step(running_state(7, 50 * S, earlier.clone()), Event::HostExited { pid: 7, code: 1 }, 60 * S);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
    // One millisecond later: the first has left the window.
    let (st, _) = step(running_state(7, 50 * S, earlier.clone()), Event::HostExited { pid: 7, code: 1 }, 60 * S + 1);
    assert_eq!(fallback_reason(&st), None);
    assert!(matches!(st.phase, Phase::Backoff { delay: 3000, .. }));
    // One millisecond earlier: inside.
    let (st, _) = step(running_state(7, 50 * S, earlier), Event::HostExited { pid: 7, code: 1 }, 60 * S - 1);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
}

#[test]
fn two_fast_crashes_edges() {
    // The earlier crash lived 3000 ms; the new one lived exactly 3000 ms: a loop. 3001 ms: not.
    for (uptime, loops) in [(FAST_CRASH_MS, true), (FAST_CRASH_MS + 1, false)] {
        let st = running_state(7, 100 * S - uptime, vec![crash(90 * S, FAST_CRASH_MS)]);
        let (st, _) = step(st, Event::HostExited { pid: 7, code: 1 }, 100 * S);
        assert_eq!(fallback_reason(&st) == Some(Reason::CrashLoop), loops, "uptime {uptime}");
    }
    // The earlier one was long-lived: not a loop.
    let st = running_state(7, 99 * S, vec![crash(90 * S, FAST_CRASH_MS + 1)]);
    let (st, _) = step(st, Event::HostExited { pid: 7, code: 1 }, 100 * S);
    assert_eq!(fallback_reason(&st), None);
    // Two fast crashes more than a minute apart are not a loop either.
    let st = running_state(7, 100 * S, vec![crash(10 * S, 100)]);
    let (st, _) = step(st, Event::HostExited { pid: 7, code: 1 }, 100 * S + 100);
    assert_eq!(fallback_reason(&st), None);
}

#[test]
fn six_crashes_in_ten_minutes_edges() {
    // Five earlier crashes, each a long run, spaced two minutes apart: 0, 120, 240, 360, 480 s.
    let earlier: Vec<Crash> = (0..5).map(|i| crash(i * 120 * S, 60 * S)).collect();
    let (st, _) = step(running_state(7, 570 * S, earlier.clone()), Event::HostExited { pid: 7, code: 1 }, 600 * S);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop), "six within exactly ten minutes");
    let (st, _) = step(running_state(7, 570 * S, earlier.clone()), Event::HostExited { pid: 7, code: 1 }, 600 * S + 1);
    assert_eq!(fallback_reason(&st), None, "the first has aged out");
    assert_eq!(st.crashes.len(), 5);
    // Four earlier ones are never enough.
    let (st, _) = step(running_state(7, 570 * S, earlier[1..].to_vec()), Event::HostExited { pid: 7, code: 1 }, 600 * S);
    assert_eq!(fallback_reason(&st), None);
}

#[test]
fn old_crashes_are_forgotten() {
    let st = running_state(7, 0, vec![crash(0, 10 * S), crash(1000, 10 * S)]);
    // A crash an hour later is the first of a new run.
    let (st, a) = step(st, Event::HostExited { pid: 7, code: 1 }, 3_600 * S);
    assert_eq!(st.crashes.len(), 1);
    assert_eq!(st.phase, Phase::Launching, "first crash: no wait");
    assert_eq!(a.last(), Some(&Action::StartHost));
}

#[test]
fn the_crash_log_is_capped() {
    let mut log: Vec<Crash> = (0..100).map(|i| crash(1_000_000 + i, 10 * S)).collect();
    prune(&mut log, 1_000_100);
    assert_eq!(log.len(), 32);
    assert_eq!(log.last().unwrap().at, 1_000_099);
}

// ---- the host that exits instantly, for ever -------------------------------------------------

/// Feeds cycles of "launch, the host exits after `uptime` with `code`" until the machine leaves the supervise loop.
fn run_cycles(mut st: State, start: Millis, uptime: Millis, code: Option<i32>) -> (State, usize) {
    let mut t = start;
    let mut cycles = 0;
    while !matches!(st.phase, Phase::Fallback(_) | Phase::Done(_)) {
        cycles += 1;
        assert!(cycles < 50, "never ends: {:?}", st.phase);
        if let Phase::Backoff { delay, .. } = st.phase {
            t += delay;
            st = step(st, Event::Tick { host_hung: false }, t).0;
        }
        match code {
            None => {
                st = step(st, Event::HostSpawnFailed, t).0;
            }
            Some(code) => {
                let pid = 100 + cycles as u32;
                st = step(st, Event::HostStarted { pid }, t).0;
                if !matches!(code, 1000) {
                    st = step(st, Event::HostReady, t).0;
                }
                t += uptime;
                st = step(st, Event::HostExited { pid, code }, t).0;
            }
        }
    }
    (st, cycles)
}

#[test]
fn a_host_that_exits_at_once_for_ever_ends_in_explorer_after_two_cycles() {
    for code in [1, -1073741819, EXIT_RESTART, 1000 /* never says ready: sentinel, exits code 1000 */] {
        let (booted, _) = boot(facts(), 5000);
        let (st, cycles) = run_cycles(booted, 5000, 0, Some(code));
        assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop), "code {code}");
        assert_eq!(cycles, 2, "code {code}");
    }
}

#[test]
fn a_host_that_cannot_even_be_started_ends_in_explorer_after_two_attempts() {
    let (booted, _) = boot(facts(), 5000);
    let (st, cycles) = run_cycles(booted, 5000, 0, None);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
    assert_eq!(cycles, 2);
}

#[test]
fn a_host_that_crashes_every_ten_seconds_is_stopped_by_the_minute_rule() {
    let (booted, _) = boot(facts(), 0);
    let (st, cycles) = run_cycles(booted, 0, 10 * S, Some(1));
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
    assert_eq!(cycles, 3);
}

#[test]
fn a_host_that_crashes_every_hundred_seconds_is_stopped_by_the_ten_minute_rule() {
    let (booted, _) = boot(facts(), 0);
    let (st, cycles) = run_cycles(booted, 0, 100 * S, Some(1));
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
    // Crashes at about 100, 200, 301, 404, 507 and 610 s (with the 1 s and 3 s backoffs): the sixth sits within ten minutes of the first.
    assert_eq!(cycles, 6);
}

#[test]
fn a_host_that_crashes_every_two_minutes_never_trips_a_rule_and_is_restarted_each_time() {
    // Five gaps of 120 s plus backoffs are longer than ten minutes, so there are never six in the window: slow flapping is allowed.
    let (st, _) = boot(facts(), 0);
    let mut st = st;
    let mut t = 0;
    for i in 0..20u32 {
        if let Phase::Backoff { delay, .. } = st.phase {
            t += delay;
            st = step(st, Event::Tick { host_hung: false }, t).0;
        }
        st = step(st, Event::HostStarted { pid: 10 + i }, t).0;
        st = step(st, Event::HostReady, t).0;
        t += 120 * S;
        st = step(st, Event::HostExited { pid: 10 + i, code: 1 }, t).0;
        assert!(!matches!(st.phase, Phase::Fallback(_)), "cycle {i}");
    }
}

#[test]
fn a_host_that_exits_zero_is_not_a_request_to_leave() {
    // Exit code 0 is what a host returns when Windows signs the user out or when it hands over to another
    // instance: it must never delete the Shell value. Only EXIT_LEAVE does.
    assert_ne!(EXIT_LEAVE, 0);
    let (st, a) = step(up(0), Event::HostExited { pid: 7, code: 0 }, 60 * S);
    assert_eq!(fallback_reason(&st), None);
    assert!(!kinds(&a).contains(&ActionKind::ClearShellValue));
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
}

#[test]
fn a_host_that_exits_zero_at_once_for_ever_ends_in_the_crash_loop_rule() {
    // Instantly and again and again: every one counts as a crash (a single-instance host that hands over and exits 0
    // would otherwise be restarted without end).
    let mut st = with_phase(Phase::Launching);
    let mut t = 10 * S;
    let mut ended = false;
    for i in 0..10 {
        st = step(st, Event::HostStarted { pid: 20 + i }, t).0;
        let (next, _) = step(st, Event::HostExited { pid: 20 + i, code: 0 }, t + 100);
        st = next;
        t += 200;
        if matches!(st.phase, Phase::Fallback(_)) {
            assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
            ended = true;
            break;
        }
        // Backoff phases are walked through so the loop reaches the next launch.
        if let Phase::Backoff { delay, .. } = st.phase {
            t += delay;
            st = step(st, Event::Tick { host_hung: false }, t).0;
        }
    }
    assert!(ended, "ten instant exits with code 0 must reach Explorer");
}

#[test]
fn explorer_already_the_shell_means_the_host_is_not_started() {
    let (st, a) = boot(Facts { explorer_is_shell: true, ..facts() }, 0);
    assert_eq!(st.phase, Phase::Done(Outcome::ExplorerHasShell));
    assert_eq!(a, vec![Action::Exit { code: 0 }]);
    // The explicit ways back still win: --panic, Safe Mode and the Shift skip do their own thing.
    let (st, _) = boot(Facts { explorer_is_shell: true, panic: true, ..facts() }, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::Panic));
    let (st, _) = boot(Facts { explorer_is_shell: true, safe_mode: true, ..facts() }, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::SafeMode));
    let (st, _) = boot(Facts { explorer_is_shell: true, shift_held: true, ..facts() }, 0);
    assert_eq!(fallback_reason(&st), Some(Reason::UserSkip));
    // A crash loop on record still ends in the (idempotent) fall-back rather than a silent exit.
    let crashes = (1..=3).map(|i| Crash { at: 100 * S - i * S, uptime_ms: 10 * S }).collect();
    let (st, _) = boot(Facts { explorer_is_shell: true, crashes, ..facts() }, 100 * S);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
}

// ---- the clock goes backwards ---------------------------------------------------------------

#[test]
fn a_backoff_does_not_freeze_when_the_clock_jumps_back() {
    let st = with_phase(Phase::Backoff { since: 10_000_000, delay: 3000 });
    // The clock is now ten thousand seconds earlier.
    let now = 100;
    let (st, a) = step(st, Event::Tick { host_hung: false }, now);
    assert!(a.is_empty());
    assert_eq!(st.phase, Phase::Backoff { since: now, delay: 3000 }, "the timer is re-based to the new clock");
    assert_eq!(st.next_wake_ms(now), Some(3000));
    let (st, a) = step(st, Event::Tick { host_hung: false }, now + 3000);
    assert_eq!(a, vec![Action::ClaimIdentity, Action::StartHost]);
    assert_eq!(st.phase, Phase::Launching);
}

#[test]
fn the_runner_never_waits_longer_than_the_timer_even_before_the_machine_noticed_the_jump() {
    let st = with_phase(Phase::Backoff { since: 10_000_000, delay: 3000 });
    assert_eq!(st.next_wake_ms(100), Some(3000));
    let st = with_phase(Phase::Starting { pid: 1, since: 10_000_000 });
    assert_eq!(st.next_wake_ms(100), Some(READY_DEADLINE_MS));
}

#[test]
fn a_starting_host_gets_a_fresh_deadline_after_a_jump_back() {
    let (st, _) = boot(facts(), 10_000_000);
    let (st, _) = step(st, Event::HostStarted { pid: 1 }, 10_000_000);
    let (st, a) = step(st, Event::Tick { host_hung: false }, 500);
    assert!(a.is_empty());
    assert_eq!(st.phase, Phase::Starting { pid: 1, since: 500 });
    let (_, a) = step(st, Event::Tick { host_hung: false }, 500 + READY_DEADLINE_MS);
    assert_eq!(a[0], Action::KillHost);
}

#[test]
fn a_host_exit_after_a_jump_back_does_not_underflow_and_counts_as_a_fast_crash() {
    let (st, _) = step(running_state(7, 5_000_000, vec![]), Event::HostExited { pid: 7, code: 1 }, 1000);
    assert_eq!(st.crashes, vec![crash(1000, 0)]);
}

#[test]
fn crashes_stamped_in_the_future_are_pulled_back_so_they_cannot_hide() {
    // The clock was set back two hours after two crashes: both count as "just now"; the third is a loop.
    let now = 50_000_000;
    let earlier = vec![crash(now + 7_200_000, 10 * S), crash(now + 7_200_100, 10 * S)];
    let (st, _) = step(running_state(7, now - 10 * S, earlier), Event::HostExited { pid: 7, code: 1 }, now);
    assert_eq!(fallback_reason(&st), Some(Reason::CrashLoop));
}

#[test]
fn crashes_from_a_wildly_wrong_clock_are_thrown_away_at_boot() {
    let now = 50_000_000;
    let mut log = vec![crash(now + FUTURE_TOLERANCE_MS + 1, 10 * S), crash(now + FUTURE_TOLERANCE_MS, 10 * S), crash(now - 1, 10 * S)];
    prune(&mut log, now);
    assert_eq!(log, vec![crash(now, 10 * S), crash(now - 1, 10 * S)]);
}

#[test]
fn time_going_backwards_during_boot_is_harmless() {
    // The file's crash is newer than "now": counted as now, not as a negative age.
    let (st, _) = boot(Facts { crashes: vec![crash(2_000_000, 10 * S)], ..facts() }, 1_000_000);
    assert_eq!(st.crashes, vec![crash(1_000_000, 10 * S)]);
}

// ---- small things ---------------------------------------------------------------------------

#[test]
fn reasons_round_trip_and_only_two_leave_the_value() {
    let all = [Reason::SafeMode, Reason::UserSkip, Reason::Panic, Reason::SelfTestFailed, Reason::UserExit, Reason::Escape, Reason::CrashLoop];
    for r in all {
        assert_eq!(Reason::parse(r.as_str()), Some(r));
    }
    assert_eq!(Reason::parse("nonsense"), None);
    let keepers: Vec<Reason> = all.into_iter().filter(|r| !r.deletes_value()).collect();
    assert_eq!(keepers, vec![Reason::SafeMode, Reason::UserSkip]);
}

#[test]
fn every_action_describes_itself() {
    let samples = [
        Action::ClaimIdentity,
        Action::StartHost,
        Action::SignalShellReady,
        Action::ReleaseIdentity,
        Action::ClearShellValue,
        Action::KillHost,
        Action::RestoreWorkAreas,
        Action::ShowTaskbar,
        Action::ProbeExplorer,
        Action::StartExplorer,
        Action::WaitExplorerUp { ms: 10_000 },
        Action::SelfTest,
        Action::WriteState(Persist::Crashes(vec![])),
        Action::ShowMessage(Message { value_left: true, explorer_down: false }),
        Action::Exit { code: 0 },
    ];
    for a in samples {
        assert!(!a.describe().is_empty());
    }
}
