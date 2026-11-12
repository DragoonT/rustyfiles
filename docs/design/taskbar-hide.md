# Hiding Explorer's taskbar while the dock runs (experimental, off by default)

Shell plan increment E1 (rows B11 to B13 of section 1.2, section 6 "the safety net"). One switch on the dock's Settings page,
"Windows görev çubuğunu gizle (deneysel)" / "Hide the Windows taskbar (experimental)", setting `shell_hide_taskbar`, default OFF. For a
user who does not ask for it nothing changes: no thread, no window, no guard process, no file is written.

The honest lines the switch shows: experimental; Windows' taskbar and its notification-area icons are hidden while the dock runs and
come back when it stops; the Windows key and Windows' own flyouts still open where the hidden taskbar was (turn on the Windows key switch as well); none of Windows' taskbar settings is changed; if rustyfiles stops unexpectedly a small helper
(`rustyfiles-shellguard.exe`) brings the taskbar back within a second or two; if the helper is not running the taskbar is not hidden.

**Nothing in this document was run against Explorer's real windows.** Every rule is tested against a pretend desktop; the real
calls are listed under "What was not run".

## What is changed, and what is not

* **Changed:** the VISIBILITY of Explorer's taskbar windows, `Shell_TrayWnd` (the primary monitor's) and every
  `Shell_SecondaryTrayWnd` (one per other monitor), with `SetWindowPos(SWP_HIDEWINDOW)` and nothing else in the flags that moves, sizes,
  reorders or activates. Only windows that were VISIBLE are hidden, and only those are ever shown again: a taskbar that something else
  hid before stays as it was found.
* **Not changed:** the taskbar's auto-hide state (`ABM_SETSTATE`), any other taskbar setting, any registry value, any Windows setting.
  Why not auto-hide: other bars use it as a first step (ManagedShell sets it, then hides), but it writes the user's taskbar settings
  (`StuckRects3`), and nobody could show that it is undone on EVERY path (a power cut after the call included). Visibility is
  process-local state of Explorer's windows: it ends with the Explorer process, and a new Explorer shows its taskbar as always.
* **The Start button window (class atom 0xC017)** of the Windows 10 taskbar is not hidden: on Windows 11 the taskbar is one XAML
  island inside `Shell_TrayWnd`; hiding the outer window hides it.
* **The work area:** see "The strip the taskbar reserved".

## The rules (all of them tested, see "Tests")

1. **The guard first.** Hiding is REFUSED unless `rustyfiles-shellguard.exe` (beside role) is running and has CONFIRMED. The host
   starts it with its own process id and two named events it created (see "The guard"). No file next to the program, no start, no
   confirmation within 3 s (and the guard is ended): the taskbar stays and the next try is after 5 s, then 15 s, then 60 s.
2. **The record before the hide.** The state file `shell-state.json` (in the settings folder; `RUSTYFILES_CONFIG_DIR` honoured) gets
   `hidden_taskbar` and `pending_restore` BEFORE the first window is hidden, every time a window is added. The record of a window is its
   handle, class, owner process id, the owner's START TIME, plus the host's pid and start time and the time of writing. A state file
   that cannot be written (or is from a newer version) means nothing is hidden.
3. **A handle is acted on only if it is still that window.** Before a window is shown (guard, host, panic hook) it must still have the
   recorded class AND be owned by the recorded process (same id AND same start time: ids are reused) AND that process must run
   `explorer.exe` from the Windows folder. The real `set_visible` checks class and owner AGAIN at the moment of the call. A window
   that fails any check is "foreign": left alone, whatever the file says. A start time that is 0 (unreadable) proves nothing and is
   treated as foreign.
4. **The guard ending shows the taskbar.** A once-a-second look (the host's timer, only while the switch is on) notices that the guard
   process ended and shows the taskbar at once; hiding stops until a guard is confirmed again (backoff 5 s, 15 s, 60 s; forgotten
   after the guard stayed up two minutes).
5. **Leaving restores, then tells.** Switch off or quit: windows shown, work areas put back, record and flag removed, only then the
   guard is told (its event) and exits. "Shown" is not the answer of the show call: the call is queued in Explorer's own thread (it
   must not wait for an Explorer that is hung), so `Ok` only means "asked". The host WAITS (half a second at most) until the windows
   are visible, and counts a window that did not come back as a failure. If a window cannot be shown the record stays and the guard
   is NOT told: it stays as the safety net.
   **A restore that failed is owed, and is retried** on the host's timer, whatever the switch says now, after 1 s, 2 s, 4 s ... up
   to 30 s apart, twelve times at most (each try may set the work area, which is broadcast to every window). When one works the guard
   is told. The thread does not end while a restore is owed (the switch going off keeps it alive until the work is done or the
   retries are used up). After the twelfth failure the host gives up, and the guard is what is left. Tests:
   `a_show_that_is_accepted_but_never_happens_is_not_a_restore_the_record_and_the_guard_stay`,
   `a_show_that_takes_effect_while_it_is_waited_for_is_a_restore`,
   `the_setting_off_with_a_lost_show_is_retried_on_later_ticks_until_it_works`,
   `the_retries_wait_longer_each_time_and_stop_after_the_limit_with_the_guard_still_there`,
   `a_lost_guard_with_a_restore_that_fails_is_retried_before_anything_is_hidden_again`.
6. **A stale record is undone first.** At the host's start (whatever the switch says) and at the guard's start (before it says "armed")
   a record written by another host process, or by one whose id was reused (start time differs), is undone: the windows it names that
   are still the same windows are shown. A record that cannot be read as a whole (`hidden_taskbar` present but garbled, or with even
   ONE entry that cannot be read: trimming it would show some windows, delete the key and orphan the rest) makes every hidden
   taskbar window of Explorer's visible (the "sweep"). Without a record and without the key nothing is touched.
   With the switch OFF the host looks once per process (a thread of its own reading the state file, nothing else) whether the file
   still holds `hidden_taskbar`; only then a one-shot worker is started that undoes it and ends (`shell::taskbar_hide::decide`,
   `Action::StaleCheck`). With the switch on, the worker that hides does the same first. Limit: two hosts sharing one settings
   folder would see each other's live record as stale; the dock is single-instance, and test copies use their own folder.
7. **Explorer restarting** (`TaskbarCreated`, heard by the host's own hidden window): the handles we knew are forgotten (never shown:
   they are gone, and had they been reused the owner check stops it), the new taskbar windows are found and hidden if the switch is
   still on and the guard is alive. Because Explorer creates the secondary taskbars a moment later, two follow-up looks run at 0.7 s
   and 3.2 s after the broadcast (and the once-a-second look would find them anyway).
8. **Monitors added or removed** (`WM_DISPLAYCHANGE`, debounced 400 ms): the windows are listed again; a new monitor's secondary taskbar
   is recorded and hidden; a record entry whose window is gone is harmless (skipped as "gone").
9. **Explorer shows its taskbar again** (it does, now and then): found by the once-a-second look and hidden again; the record already
   names it.
10. **No Explorer taskbar, no guard.** When Explorer does not own the shell window (`rf_win::shellhost::mode::detect` is not Beside:
    the future shell mode) there is no Explorer taskbar and nothing is started.

## The guard (`rustyfiles-shellguard.exe --beside-guard <host pid> --ready-event <name> --done-event <name> --state-dir <dir>`)

The beside role existed before this increment (it showed the taskbar by class name when the host died). Now:

* The host creates two manual-reset events in the session's namespace, `Local\rustyfiles-beside-ready-<host pid>-<n>` and
  `Local\rustyfiles-beside-done-<host pid>-<n>` (n counts the guards of this PROCESS, not of one run of the feature: the first
  version counted from 1 at every switch-on, so a guard still ending could hold the names of the next one), and starts the guard
  with the names. `CreateEventW` hands out the existing object for a name in use, so the host checks `ERROR_ALREADY_EXISTS` and
  refuses such an event (the start then fails and the taskbar stays). Names that do not start with `Local\`, or are equal, are
  refused by the guard.
* The guard is started with `CREATE_BREAKAWAY_FROM_JOB` first (a job closed with kill-on-close, as some launchers and terminals use,
  would otherwise end the guard together with the host), and without it when the job forbids leaving. It is still the host's CHILD:
  Task Manager's "End process tree" ends it too (see the table).
* The guard opens both (ready: set rights; done: wait rights), checks the host process, undoes a stale record (rule 6), and only then
  sets `ready`. Then it blocks on the host's PROCESS HANDLE and the `done` event (no polling).
* Host dies, for any reason: the guard shows what the record names (validated as in rule 3) in `SWP_ASYNCWINDOWPOS` calls that return
  at once (a hung or restarting Explorer cannot hold the guard), looks for half a second that they are visible, puts the work areas
  back, clears `pending_restore` and the record, and exits 0. If nothing could be found it tries again every second, five times;
  if a window of the record could not be shown it ends with 1 and the flag stays.
* `done` set: it exits 0 and looks at NOTHING (the host may already have hidden the taskbar again for a new guard; that guard's record
  is not the old guard's to undo).
* The events cannot be opened: exit 3, no confirmation, so the host hides nothing. The confirmation cannot be sent: exit 4.
* Session ending (`CTRL_LOGOFF`/`CTRL_SHUTDOWN`): exits quietly, touches nothing.
* A host that is already gone when the guard starts: the record, if any, is undone; no confirmation (there is nobody to confirm to).
* Without a usable record (never written, or garbled) the guard falls back to the sweep: every HIDDEN taskbar window of Explorer's.
  This is the beside role's behaviour from before this increment, kept on purpose: it errs towards a visible taskbar, the guard
  exists solely when the user asked for hiding, and "stays as it was found" is the rule for a record that is readable. The one
  case it touches a taskbar that another tool hid is a host that dies before it hid anything while that tool's taskbar is hidden;
  showing it is the harmless direction.
* Manual rescue, useful in the first try: `rustyfiles-shellguard.exe --beside-guard 999999 --state-dir <settings folder>` (a process id
  that does not exist counts as "the host is gone") does exactly the restore above and exits.
* Log: `shell-guard.log` in the settings folder (200 KB, then `.old`).

## Every way of losing the taskbar, and the answer (each one a named test)

| Way of losing it | Answer | Test |
|---|---|---|
| the host crashes / is killed with the taskbar hidden | the guard shows the recorded windows, clears the flag and record | `tbbeside_tests::the_host_crashes_with_the_taskbar_hidden_the_guard_shows_exactly_the_recorded_windows`, `host_and_guard_together_a_crash_of_the_host_after_hiding_brings_the_taskbar_back` |
| Task Manager ends the host, work areas changed | work areas back BEFORE the flag is cleared | `the_host_is_killed_in_task_manager_and_the_work_areas_come_back_before_the_record_is_cleared` |
| the guard is killed | the host shows the taskbar at the next tick, hides no more, restarts the guard after a pause | `tbctl_tests::the_guard_ending_while_hidden_brings_the_taskbar_back_at_the_next_tick_and_hiding_stops`, `a_lost_guard_is_replaced_after_a_pause_and_hiding_resumes`, `a_guard_that_keeps_dying_waits_longer_each_time` |
| the power goes / a crash with nobody left (record stays) | the next host start (any setting) or next guard start undoes it | `the_controllers_startup_undoes_a_record_left_by_a_crashed_host_whatever_the_setting_is` (the controller), `shell::taskbar_hide::the_stale_check_starts_the_worker_only_for_a_state_file_that_holds_a_record` (the thread and the order), `a_stale_record_of_an_earlier_host_is_undone_before_the_guard_says_it_is_armed` |
| process id reused | start time differs: stale; and a window of a reused id is foreign | `a_record_of_a_reused_process_id_is_also_stale`, `a_reused_process_id_makes_the_record_stale_too`, `a_pid_reused_by_another_program_window_is_not_hidden_or_shown` |
| a reboot: the old handle numbers belong to the new Explorer | nothing foreign is touched, the stale record is dropped | `after_a_reboot_the_old_handles_are_gone_and_nothing_foreign_is_touched`, `a_record_that_names_windows_of_another_owner_leaves_them_alone_and_the_guard_still_finishes` |
| the state file is garbled | the sweep | `an_unreadable_record_shows_every_hidden_explorer_taskbar`, `a_garbled_record_falls_back_to_showing_every_hidden_taskbar_of_explorer` |
| the state file cannot be written | nothing is hidden | `nothing_is_hidden_when_the_state_file_cannot_be_written` |
| the guard is missing / does not start / does not confirm | nothing is hidden | `nothing_is_hidden_when_the_guard_is_missing`, `..._does_not_start`, `..._does_not_confirm_and_it_is_ended` |
| the switch goes off / the host quits | windows shown, record removed, then the guard is told | `the_setting_going_off_shows_the_taskbar_clears_the_record_and_then_tells_the_guard`, `quitting_the_host_does_the_same`, `host_and_guard_together_a_clean_exit_leaves_nothing_for_the_guard_to_do` |
| a window cannot be shown at quit | record stays, guard stays | `a_window_that_cannot_be_shown_keeps_the_record_and_leaves_the_guard_to_finish` |
| Explorer is hung or drops the queued show at quit / switch-off | the host waits for visibility, finds the windows still hidden, keeps the record and the guard, retries on the timer | `a_show_that_is_accepted_but_never_happens_is_not_a_restore_the_record_and_the_guard_stay`, `the_setting_off_with_a_lost_show_is_retried_on_later_ticks_until_it_works` |
| the restore fails when the switch goes off (or when the guard was lost) | owed: retried with growing pauses, independent of the switch, until it works or twelve tries are used; the guard stays meanwhile | `the_retries_wait_longer_each_time_and_stop_after_the_limit_with_the_guard_still_there`, `a_lost_guard_with_a_restore_that_fails_is_retried_before_anything_is_hidden_again` |
| a crashed host AND a dead guard (End process tree, power cut), switch off at the next start | the one-shot stale check undoes the record | `shell::taskbar_hide::the_wiring_starts_hides_turns_off_and_looks_for_a_stale_record_once_when_the_setting_is_off`, `a_record_in_the_state_file_is_seen_whether_or_not_it_can_be_read`, `the_controllers_startup_undoes_a_record_left_by_a_crashed_host_whatever_the_setting_is` (the controller), `shell::taskbar_hide::the_stale_check_starts_the_worker_only_for_a_state_file_that_holds_a_record` (the thread and the order) |
| the record is only partly readable | the sweep (nothing orphaned) | `a_record_that_is_only_partly_readable_is_swept_whole_so_no_recorded_window_is_orphaned` |
| the guard's events were taken by an older guard (names reused) | refused (`ERROR_ALREADY_EXISTS`), names counted per process | `an_event_name_that_is_in_use_is_refused_not_shared`, `the_guard_counter_belongs_to_the_process_so_a_new_world_never_reuses_a_name` |
| **NOT answered:** the host is alive but hung (a deadlock) | nothing: the guard waits on the process handle only, there is no heartbeat. The user can still reach Task Manager (Ctrl+Shift+Esc) and end the host; the guard then restores. A heartbeat would need a poll in the guard and a decision about what "hung" means; left for a decision. | - |
| **NOT answered:** "End process tree" on the host ends the guard (its child) too | the record stays; the next start of the host (any switch value) undoes it; by hand: the rescue command below, or restart Explorer from Task Manager | the stale-record rows above |
| a panic in the host | the panic hook shows every hidden taskbar window of Explorer's (queued, returns at once) before unwinding; a panic inside the controller is caught, the taskbar shown, the feature stops | code in `rf_win::taskbar_hide` (`install_panic_restore`, `Ctx::guarded`); the sweep itself: `tbmodel::the_sweep_shows_only_hidden_taskbars_of_explorer` |
| Explorer restarts | old handles forgotten, new ones hidden | `explorer_restarting_forgets_the_old_handles_and_hides_the_new_taskbar_when_still_wanted` |
| a monitor comes or goes | recorded, then hidden / skipped | `a_new_monitor_gets_its_secondary_taskbar_recorded_and_then_hidden`, `a_monitor_that_went_away_leaves_a_harmless_entry_that_is_skipped_at_restore` |
| Explorer shows its taskbar again | hidden again | `explorer_showing_the_taskbar_again_is_hidden_again_at_the_next_tick` |
| the session ends | guard exits touching nothing; the host's window restores on `WM_ENDSESSION` | `the_session_ending_leaves_everything_as_it_is` |
| the old guard hears "done" after the new record exists | exits without looking | `told_means_done_even_if_a_record_has_already_been_written_for_the_next_guard` |

## The strip the taskbar reserved (the work area)

**Not determined without running:** whether a hidden `Shell_TrayWnd` keeps its strip out of the work area. Other bars report that it
does (the work area is Explorer's own bookkeeping of the registered toolbars, the taskbar being one; hiding its window does not
unregister it). With the switch on the user then has the dock floating above an empty strip of the taskbar's height, and maximised
windows stop above that strip.

**What was chosen:** the least persistent way, and only on request.

* By default (`RUSTYFILES_TASKBAR_WORKAREA` not set) the work area is not touched at all. The strip stays reserved. This is the first
  step of the first careful try.
* With `RUSTYFILES_TASKBAR_WORKAREA=1` (second step): `SystemParametersInfo(SPI_SETWORKAREA)` WITHOUT `SPIF_UPDATEINIFILE`, i.e. the
  RUNNING SESSION's work area, not a stored value (Explorer recomputes it whenever a toolbar changes or it restarts; nothing is
  written to the profile). It is changed only where the geometry is unambiguous: the monitor's work area ends exactly (±1 px) where
  the hidden taskbar begins, on any of the four edges; then it grows to the monitor's edge (`tbmodel::giveback`). When another
  toolbar lies between (our own dock's reserved band does when `dock_reserve` is on, which is the default) nothing is changed: which
  toolbar lies outside which cannot be told without running. So with the defaults the strip stays and the second step does nothing;
  with `dock_reserve = false` it gives the strip back.
* The user's own rectangles are written to `original_work_areas` (a rectangle saved by an unfinished run is kept, never overwritten by
  an already changed one) and `pending_restore` is set BEFORE the first one is changed. They are put back on setting off, on quit,
  by the guard on host death, and the guard's own start-up restore. Explorer taking the strip back is answered (debounced 300 ms) at
  most four times a minute, then left alone, so the two cannot fight for ever.
* The rectangles are PHYSICAL pixels (the host measures them on a per-monitor-aware thread). The guard has no DPI manifest, so
  `workarea::set_work_area` makes its thread per-monitor-aware for the one call and puts it back; without that, display scaling other
  than 100 percent would have the guard restore a work area from coordinates Windows reads as virtualised.
* Not done: moving the dock's own band (it asks `ABM_QUERYPOS` for the whole bottom edge and Explorer takes away what toolbars hold;
  whether a hidden taskbar still counts is exactly the unknown), and `ABM_SETSTATE` (see above).

The probe prints what the rule WOULD do on this machine (run it before anything else).

## Read-only probe

`cargo run --offline -p rf-shellguard --example taskbar_probe` prints the monitors (rectangle and work area), the taskbar-class
windows (class, visible, owner is Explorer, rectangle, monitor index), how many it would hide, what the work-area rule would do, and
whether the state file holds a record. Counts and rectangles only. It calls `EnumWindows`, `GetClassNameW`, `GetWindowRect`,
`IsWindowVisible` and a query-only `OpenProcess`; no window is changed or messaged (a test keeps examples and tests from naming the
functions that could). On this machine (build 26300, two monitors): 2 windows, both visible, both owned by Explorer; with the dock not
running the rule would give back both 48 px strips.

## Where things are

| File | Role |
|---|---|
| `rf-shellguard/src/tbmodel.rs` | pure: window identity, `judge`, `plan_restore`, `plan_sweep`, `is_stale`, the gate `may_hide`, `giveback`, `Throttle` |
| `rf-shellguard/src/tbctl.rs` | pure: the host's `Controller` over the `HideWorld` trait |
| `rf-shellguard/src/tbfakes.rs`, `tbctl_tests.rs`, `tbbeside_tests.rs` | the pretend desktop and the named tests |
| `rf-shellguard/src/beside.rs` | the guard's side: handshake, stale restore, told, restore |
| `rf-shellguard/src/statefile.rs` | `hidden_taskbar` (get/set/clear), unknown keys survive the guard's rewrites |
| `rf-shellguard/src/win/tbwin.rs` | THE ONLY FILE THAT HIDES OR SHOWS A REAL WINDOW (and the real list / process queries) |
| `rf-shellguard/src/win/link.rs`, `win/workarea.rs` | the guard's end of the events; the work-area setter |
| `rf-shellguard/tests/tb_static.rs`, `real_link.rs`, `examples/taskbar_probe.rs` | source scans; the events against events the test creates; the probe |
| `rf-win/src/taskbar_hide.rs` | the host's thread, its hidden window (TaskbarCreated, display, work area, end of session), the real world, the panic net |
| `rf-app/src/shell/taskbar_hide.rs` | follows the setting (`decide`: start, turn off, the one-shot stale check); three one-line hooks in `dock/host.rs` |

## Tests

`cargo test --offline -p rf-shellguard` (the controller, the guard, the state file, the rules, the static scans, the events),
`cargo test --offline -p rf-win --lib taskbar_hide` and `-p rf-app --lib shell::taskbar_hide`. No test starts the thread or the
window, hides, shows or messages a window of Explorer's, writes the registry or changes a Windows setting; temp folders only.

## What was not run (the real paths)

* `tbwin::set_visible` against Explorer's real windows (hide and show), and the half-second look for visibility after it.
* `taskbar_hide::TaskbarHider` (thread, hidden window, timer, `TaskbarCreated`, `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE`,
  `WM_ENDSESSION`, the thread that stays alive for an owed restore), the real world (starting the guard with and without the
  breakaway flag, the events, waiting for the confirmation), `install_panic_restore`, the one-shot stale check's worker.
* `tbwin::wait_visible` (the half-second look that decides whether a restore happened) and `SetThreadDpiAwarenessContext` around
  `SPI_SETWORKAREA`.
* The guard started by the host with the real events and a real host process (`real_link.rs` runs the events alone).
* `SPI_SETWORKAREA` (apply and restore).
* Everything about Explorer's reaction: see the questions below.

## Questions only a run can answer (the lead's checklist)

1. Does `SW_HIDE` of the outer `Shell_TrayWnd` really remove the XAML taskbar from the screen (the island has its own composition), on
   both monitors, and does it stay hidden (how often does Explorer show it again: the once-a-second look is the answer)?
2. Does a hidden taskbar still reserve its strip (look at the probe's work area, or maximise a window)?
3. Where do Windows' own flyouts open (Start, Win+A, the clock, the volume) when their anchor is hidden? They are anchored by
   Explorer to the taskbar's position. The Windows key opens Windows' Start there unless `shell_win_key` is on; the switch's text
   says so and suggests turning that one on as well. Not determined: whether the flyouts are placed oddly or partly off-screen.
4. Does the notification area matter: with the taskbar hidden its icons are out of sight and the overflow cannot be opened. Without
   the tray host (shell plan D2, not built) the user has no way to reach them. Say so to the user who turns the switch on.
5. Win+D, Win+B, Win+T, full-screen applications, Aero Peek and hover previews of the hidden window.
6. Does `TaskbarCreated` arrive at a window that is not shown (it should: broadcast to top-level windows) and are the follow-ups at
   0.7 s / 3.2 s enough for the secondary taskbars?
7. With the dock's `dock_reserve` on, does the dock's band move down or stay above the empty strip? (Plan: unchanged.)
8. `EVENT_OBJECT_SHOW` as a replacement for the one-second timer.

## The steps of a first careful try (for a human)

A reminder about the keyboard: with the taskbar hidden the Windows key still opens Windows' Start at the hidden taskbar's place. For a
useful try turn on `shell_win_key` in the same settings file (see shell-keys.md for its own first steps, `RUSTYFILES_KEYS_LOG_ONLY=1`
first), or accept that Start is reached only by the dock's own logo.

Everything below uses a TEST COPY with its own settings folder. It hides the REAL taskbar of the screen you are looking at, and that
is the test. Keep Task Manager (Ctrl+Shift+Esc) open before you start.

1. **Read-only first.** `cargo run --offline -p rf-shellguard --example taskbar_probe`. Expect 2 taskbar windows (one per monitor),
   both visible and owned by Explorer.
2. **Build both programs**: `cargo build --offline -p rf-shellguard -p rf-app` (the guard must sit next to `rustyfiles.exe`; ask the
   rustbuilder session first while it measures).
3. **A scratch settings folder**: make `%TEMP%\rf-e1`, put `settings.toml` in it with the line `shell_hide_taskbar = false`.
4. **Start the host with the switch OFF**: environment `RUSTYFILES_CONFIG_DIR=%TEMP%\rf-e1`, `RUSTYFILES_MULTI_INSTANCE=1`,
   `RUSTYFILES_MUTE=1`, `RUST_LOG=info`; run `target\debug\rustyfiles.exe --shell`. The dock appears; the Windows taskbar stays.
5. **Turn it on** by changing the line to `shell_hide_taskbar = true` (the host follows the file). Within about a second: a
   `rustyfiles-shellguard.exe` process appears, `%TEMP%\rf-e1\shell-state.json` has `hidden_taskbar` with two windows and
   `pending_restore: true`, and the Windows taskbar is gone on both monitors. `shell-guard.log` says `armed`.
6. **Kill the host (not Explorer).** In Task Manager end the test copy's `rustyfiles.exe` (pick the right pid: your real dock may run
   too). Within a second or two the taskbar is back, the record and flag are gone from the file, the guard has exited.
7. Start it again with the switch on and **kill the guard** instead: the taskbar returns within a second; after about 5 s a new guard
   starts and hides again (the log says so).
8. **Turn the switch off** in the file: taskbar back at once, guard gone.
   Also try, with the switch on, ending the host AND the guard together ("End process tree" on the host in Task Manager): the
   taskbar stays hidden and the record stays. Start the test copy again, with the switch OFF: within a second the log says it found
   a record of an earlier run, and the taskbar is back.
9. Only if all of that was right: set `RUSTYFILES_TASKBAR_WORKAREA=1` and repeat step 5; maximise a window (does it use the strip?);
   switch off and see that the original work area is back (the probe shows it).
10. Optional, with your OK: restart Explorer from Task Manager (Windows Explorer > Restart) with the switch on: the new taskbar is
    hidden again within a few seconds.
11. **If anything is stuck:** `target\debug\rustyfiles-shellguard.exe --beside-guard 999999 --state-dir %TEMP%\rf-e1` (restores from the
    file and exits); or Task Manager > Windows Explorer > Restart (a new Explorer shows its taskbar).

Remember the cost: with the Windows taskbar hidden its notification icons are out of sight, and rustyfiles has no tray of its own yet.
