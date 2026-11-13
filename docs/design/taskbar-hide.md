# Hiding Explorer's taskbar while the dock runs (experimental, off by default)

Shell plan increment E1 (rows B11 to B13 of section 1.2, section 6 "the safety net"). One switch on the dock's Settings page,
"Windows görev çubuğunu gizle (deneysel)" / "Hide the Windows taskbar (experimental)", setting `shell_hide_taskbar`, default OFF. For a
user who does not ask for it nothing changes: no thread, no window, no guard process, no file is written.

While the hide is in force the dock also takes the bottom edge of the screen (the strip the hidden taskbar still reserved goes to the
dock: see "The strip the taskbar reserved"). That is the behaviour of the switch itself; there is no environment variable for it.

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
* **The work area:** the running session's, while the hide is in force: see "The strip the taskbar reserved". Never stored (no
  `SPIF_UPDATEINIFILE`); the user's own rectangles are written down first and put back on every way out.

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
5. **Leaving restores, then tells.** Switch off or quit: windows shown, work areas put back, the dock registers its own band with
   Explorer again, record and flag removed, only then the guard is told (its event) and exits. "Shown" is not the answer of the show call: the call is queued in Explorer's own thread (it
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
| the hide is asked for but does not happen (Explorer busy; the call is queued) | the hide is not CONFIRMED: no work area is written down or set, the dock keeps its band; the next tick hides again | `tbedge_tests::nothing_is_set_while_the_hide_is_not_confirmed`, `a_hide_that_takes_effect_while_it_is_waited_for_is_a_confirmed_hide` |
| the dock does not let go of its band | after 3 s it gets its band back, nothing is set, the try is made again after 5 s, 15 s, 60 s | `tbedge_tests::a_dock_that_never_lets_go_gets_its_band_back_and_is_asked_again_after_a_pause` |
| the originals cannot be written | nothing is changed (not the dock either), tried again after the pause | `tbedge_tests::originals_that_cannot_be_written_mean_the_dock_is_not_touched_and_the_try_is_made_again_later` |
| the dock says it let go but Explorer did not take the removal (the tray host's route) | the work area still holds the band: nothing is set, after 3 s the dock gets its band back, tried again later | `tbedge_tests::a_removal_that_explorer_did_not_take_sets_nothing_and_the_dock_gets_its_band_back` |
| the hider starts before the dock has said what its band is | the taskbar is hidden, the edge waits for the dock's word and is then taken once with the right band | `tbedge_tests::nothing_is_taken_until_the_dock_has_said_what_its_band_is` |
| a monitor becomes ours after the edge was taken (plugged in, its taskbar hidden by a later pass) | its work area is written down before the first change; if that fails it is not touched | `tbedge_tests::a_monitor_that_becomes_ours_after_the_edge_was_taken_is_written_down_before_its_work_area_is_set`, `a_new_monitor_whose_original_cannot_be_written_is_not_touched` |
| resolution, scale or monitors change while hidden | at the way out the written-down rectangles are checked against the monitors as they are: worked out again from the taskbar, or left out for a monitor that has gone | `tbwork::tests::a_monitor_that_changed_resolution_gets_a_work_area_worked_out_from_its_taskbar_not_the_old_rectangle`, `tbedge_tests::a_resolution_change_while_hidden_is_not_answered_with_a_rectangle_of_the_old_screen`, `a_monitor_that_was_unplugged_while_hidden_gets_no_work_area_and_its_rectangle_lands_nowhere_else` |
| Explorer restarts and the new taskbar cannot be hidden | no taskbar of ours is hidden: the dock gets its band back at once, the edge is taken again when a hide goes through | `tbedge_tests::a_taskbar_that_cannot_be_hidden_after_an_explorer_restart_gives_the_dock_its_band_back`, `the_same_while_the_dock_is_still_letting_go` |
| the taskbar is at the top or a side, or an auto-hiding one is up | the bar at the top or a side: hidden, edge and work areas left alone; the auto-hiding one: the whole monitor is what is written down | `tbwork::tests::a_taskbar_at_the_top_or_a_side_is_left_alone`, `tbedge_tests::a_taskbar_on_the_left_is_hidden_but_the_edge_and_the_work_areas_are_left_alone`, `an_auto_hiding_taskbar_that_is_up_is_put_back_as_the_whole_monitor` |
| the host crashes with the edge taken (the work area is ours) | the guard shows the taskbar and sets the written-down rectangles (the monitor less the taskbar's strip, no band of the dock's: the dock died with the host) | `tbbeside_tests::host_and_guard_together_a_crash_of_the_host_with_the_edge_taken_gives_the_users_work_areas_back` |
| a work area that cannot be put back, or a show that never happens, at switch-off | record, edge and guard stay; retried | `tbedge_tests::work_areas_that_cannot_be_put_back_keep_the_record_the_edge_and_the_guard`, `a_show_that_never_happens_leaves_the_edge_the_dock_and_the_guard_as_they_are` |
| **NOT answered:** the host is alive but hung (a deadlock) | nothing: the guard waits on the process handle only, there is no heartbeat. The user can still reach Task Manager (Ctrl+Shift+Esc) and end the host; the guard then restores. A heartbeat would need a poll in the guard and a decision about what "hung" means; left for a decision. | - |
| **NOT answered:** "End process tree" on the host ends the guard (its child) too | the record stays; the next start of the host (any switch value) undoes it; by hand: the rescue command below, or restart Explorer from Task Manager | the stale-record rows above |
| a panic in the host | the panic hook shows every hidden taskbar window of Explorer's (queued, returns at once) before unwinding; a panic inside the controller is caught, the taskbar shown, the feature stops | code in `rf_win::taskbar_hide` (`install_panic_restore`, `Ctx::guarded`); the sweep itself: `tbmodel::the_sweep_shows_only_hidden_taskbars_of_explorer` |
| Explorer restarts | old handles forgotten, new ones hidden | `explorer_restarting_forgets_the_old_handles_and_hides_the_new_taskbar_when_still_wanted` |
| a monitor comes or goes | recorded, then hidden / skipped | `a_new_monitor_gets_its_secondary_taskbar_recorded_and_then_hidden`, `a_monitor_that_went_away_leaves_a_harmless_entry_that_is_skipped_at_restore` |
| Explorer shows its taskbar again | hidden again | `explorer_showing_the_taskbar_again_is_hidden_again_at_the_next_tick` |
| the session ends | guard exits touching nothing; the host's window restores on `WM_ENDSESSION` | `the_session_ending_leaves_everything_as_it_is` |
| the old guard hears "done" after the new record exists | exits without looking | `told_means_done_even_if_a_record_has_already_been_written_for_the_next_guard` |

## The strip the taskbar reserved (the work area): the dock takes the bottom edge

**What the first run on the user's PC determined (2026-11-13, 2560x1440 at 100 percent, Explorer running).** A hidden taskbar KEEPS its
strip. With the taskbar hidden the primary work area was 2560x1314: 48 px for the taskbar's strip plus 78 px for the dock's own band
above it, so the dock floated 46 px above the screen's bottom with the wallpaper showing under it. Both taskbar windows were hidden, the
guard ran, and killing the dock brought the taskbars back in 110 ms. The user: "altta boşluk var hala"; "doğru çözümü yapalım".

**What the switch does now.** While OUR hide is in force and CONFIRMED, the dock takes the bottom edge:

* the primary monitor's work area is the monitor less the dock's own band when `dock_reserve` is on, and the whole monitor when it is
  off; on every other monitor whose taskbar we hid it is the whole monitor (`rf_shellguard::tbwork::target_for`). Session only;
* the dock's islands are placed from the monitor's bottom edge (`rf_win::dock::system::take_edge`: the dock's band is the bottom strip
  of the monitor, an empty one with `dock_reserve` off), the same way the keeper for a session with NO Explorer places them
  (`set_own_band`);
* the dock's own desktop-toolbar registration with Explorer (`dock/band.rs`) is given back and STAYS given back while the edge is ours
  (a second band on top of ours would be a second strip), and registered again when the taskbar is shown again;
* "the monitor less a band" is one rule for the two owners of the work area (`tbwork::monitor_less_band`; `shellhost::workarea::target`
  calls it). The keeper for NO Explorer (`shellhost/workarea.rs`, behind `RUSTYFILES_SET_WORKAREA=1`) is unchanged and not merged with
  this: with no shell window it is the owner, beside Explorer the hide's controller is.

**Where it applies, and where it does not** (`tbwork::plan_edge`, tested). Only a monitor whose taskbar we hid, only a taskbar on the
BOTTOM edge (Windows 11 has no other; a bar at the top or a side is left alone, and the dock keeps its own registration), and only
where the work area there is what a hidden taskbar leaves (the monitor less its strip, within one pixel), the whole monitor (Explorer
did not reserve the strip), or, on the primary monitor, the monitor less the strip less a band cut from the bottom (the dock's
registration, not yet given back). Anything else means another toolbar reserves room there (a third-party bar, a sidebar): that monitor
is left alone, the strip stays, a warning says so once, and the dock keeps its own registration if it is the primary monitor that is
concerned. What is written down as the user's own work area is what the taskbar's strip leaves, except when the work area was read as
the whole monitor (an auto-hiding taskbar that happens to be up reserves nothing): then the whole monitor is written down, not a
guessed strip.

**The order of the steps** (`tbctl`, the named tests are in `tbedge_tests.rs`):

On the way in: guard confirmed, record written, windows hidden, the hide CONFIRMED (`wait_hidden`: the call is queued in Explorer's
thread, so `Ok` is only "asked"), the user's own work areas written to `original_work_areas` with `pending_restore` (what the user has
WITHOUT the dock: the monitor less the taskbar's strip, never a band of ours), the dock told to take the edge and let go of its band,
the work areas set once the dock has let go AND the work area shows that Explorer really dropped the band (a look every 100 ms; no
blocking wait on the host's thread, which must answer Explorer's broadcasts meanwhile), at most 3 s, else the dock gets its band back and
the try is repeated after 5 s, 15 s, 60 s. The dock's own word ("registration removed") is only what it SENT: with the tray host on its
messages go to Explorer through the host's window, and a removal that Explorer did not take would leave the band in the work area, so a
work area set then would be a second strip. While the primary monitor's work area is still the monitor less the strip less a band
(`MonitorPlan::band_held`), nothing is set. NOTHING is set, or written down, for a window that was only asked to hide, and the edge is
not taken at all until the dock has said what its band is (`Config::band_known`; "no band" is also a word): a hider that starts before
the dock would otherwise give the whole monitor to maximised windows over the dock and set the work area a second time when the band
arrives.

A monitor that becomes ours LATER (plugged in, its taskbar hidden by a later pass) has its own work area written down before the first
change to it (`write_down_originals`, in front of every set; the first record of a monitor is kept). If that cannot be written nothing
is set on it. Explorer restarting with a new taskbar that cannot be hidden (the hide is lost again) leaves no taskbar of ours hidden:
the dock then gets its band back at once (`release_unheld_edge`) instead of floating, with no band, over a visible taskbar; the edge is
taken again when a hide goes through.

On every way out (switch off, quit, the end of the session, the guard lost, a retry of an owed restore): windows shown and confirmed
visible, the written-down work areas put back - each checked against the monitors and taskbars as they are by then
(`tbwork::work_restore`): a rectangle that is still what that monitor's taskbar leaves (or the whole monitor) is put back as written;
after a resolution, scale or taskbar-size change it is worked out again as the monitor less the taskbar's strip; the rectangle of a
monitor that has gone is left out, so that it never lands on another monitor; with no monitor listed at all the rectangles go back as
written - then the dock registers its band with Explorer again (so Explorer computes the band's
place from the restored area, not the other way round), then the record and the originals are removed, then the guard is told. Any step
that fails keeps everything after it where it is and the restore is owed (retried, growing pauses). If the host dies the guard shows
the windows and sets the written-down rectangles (checked the same way, against the monitors it reads itself on a per-monitor-aware
thread); the dock's registration died with the host.

While the edge is ours: a work area that Explorer takes back (it recomputes it when a toolbar changes, on a display change, after a
restart) is answered after 300 ms, at most four times a minute, and then left alone so the two cannot fight for ever. The dock's
band changing (its size, `dock_reserve`, the thin dock's band while a window is maximised) moves the edge and the work area (`set_band`); that set is the dock's own word, not Explorer's doing, so it is not held back by the four a minute and does not use up one of them (the thin dock changes the band at most every two seconds). `TaskbarCreated` that is not an Explorer restart
(our own tray host broadcasts it when it starts and stops) no longer makes the controller forget the windows it hid; before, the taskbar
stayed hidden and unaccounted for (`explorer_restarted` now keeps what is still the same window).

**The four combinations of (taskbar hidden by us) x (tray host on)** and who sets the band:

| hidden by us | tray host | the dock's band | the work area | state |
|---|---|---|---|---|
| no | off | the dock registers with Explorer (`ABM_*`, as always) | Explorer's | as before this increment |
| no | on | the dock registers with Explorer; its messages land in the tray host's window (the first `Shell_TrayWnd`) and are passed on untouched; a `TaskbarCreated` of our own tray host no longer makes the dock remove and register again | Explorer's | the route is NOT verified (see below) and nothing here changes it; `appbar_own` and `appbar_new_refused` in the host's counts show what became of the dock's messages |
| yes | off | none registered; the host owns the edge | the host's (`tbwork`) | by construction; real run: the lead |
| yes | on | none registered; the one message that still goes through the tray host is the removal of an earlier registration, and its effect is READ from the work area, not assumed (`band_held`): a removal that did not take means nothing is set and the dock gets its band back | the host's | by construction and by the work-area check; real run: the lead (this is the combination in which the band was lost: 2560x1392 and the dock at the very bottom) |

**What the tray host does with the dock's toolbar messages (read from the code).** `SHAppBarMessage` sends to the first window of class
`Shell_TrayWnd`, which is the tray host's window while it is on (the counter `appbar` was 30 in the trial: they did arrive). The host
passes a `dwData = 0` block to Explorer's window UNTOUCHED, before any parse of its own, and returns Explorer's answer; it does not look
at who the sender is. By the code the dock's messages therefore reach Explorer's real tray window, and the reading "Explorer refuses
them" is NOT supported: `explorer_refused` (11 in the trial) counts refused ICON requests only, and nothing counted what Explorer
answered to an appbar block. What the code does show is a churn that the tray host's broadcasts caused: each start or stop of the host
broadcasts `TaskbarCreated`, the dock took that for an Explorer restart and answered it with a new round of `ABM_NEW` / `ABM_REMOVE` /
`ABM_NEW` / `ABM_QUERYPOS` / `ABM_SETPOS` (five messages, through the tray host, with the work area briefly without the band), and
only that round could lose the band. `dock::system` now keeps the registration when Explorer's taskbar window is provably the same one
(`band::announcement_is_restart`, tested); the cause of the lost band was not shown by a run, and the combination (not hidden, tray host
on) stays unverified until the lead runs it. Two new counts, `appbar_own` (the dock's own messages that came through the tray host) and
`appbar_new_refused` (registrations Explorer answered with 0), are there to show it.

*A reading that was proposed and is NOT adopted:* that the block the dock's `SHAppBarMessage` builds is made for the tray host's own
process (the first `Shell_TrayWnd`), so that Explorer cannot use the shared memory it names, and that the host must therefore build a
new block for Explorer. Nothing in the code or in the trial shows it, the block carries the SENDER's process id exactly so that the
receiver can reach back into it, and rebuilding a block blind (`SHAllocShared` and its undocumented siblings) would risk the route that
may be working. What is done instead is route-agnostic: the host never trusts the dock's word that a registration is gone, it reads the
work area (above). If the lead's run shows `appbar_new_refused` growing with the tray host on and the taskbar not hidden, rebuilding
the block in the host is the next step, and it is a change to `trayhost` alone.

**Cost.** Switch off (the default): nothing, no thread, no window, no file. Switch on: one extra pass over the monitors per second while
the hide is in force (`EnumDisplayMonitors`, a few small calls), one `EnumWindows` pass per `TaskbarCreated` heard by the dock.

The probe prints what the rule WOULD do on this machine with a 78 px band and with none (run it before anything else).

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
| `rf-shellguard/src/tbmodel.rs` | pure: window identity, `judge`, `plan_restore`, `plan_sweep`, `is_stale`, the gate `may_hide`, `originals_to_save`, `Throttle` |
| `rf-shellguard/src/tbwork.rs` | pure: the work area while our hide is in force: `monitor_less_band`, `target_for`, `plan_edge` (what is there on each monitor, what is wanted, what the user has without us, whether Explorer still holds a band of the dock's), `work_restore` (what of the written-down rectangles may be put back on the screen as it is now) |
| `rf-shellguard/src/tbctl.rs` | pure: the host's `Controller` over the `HideWorld` trait |
| `rf-shellguard/src/tbfakes.rs`, `tbctl_tests.rs`, `tbedge_tests.rs`, `tbbeside_tests.rs` | the pretend desktop (it models Explorer's work area and the dock's registration) and the named tests |
| `rf-shellguard/src/beside.rs` | the guard's side: handshake, stale restore, told, restore |
| `rf-shellguard/src/statefile.rs` | `hidden_taskbar` (get/set/clear), unknown keys survive the guard's rewrites |
| `rf-shellguard/src/win/tbwin.rs` | THE ONLY FILE THAT HIDES OR SHOWS A REAL WINDOW (and the real list / process queries) |
| `rf-shellguard/src/win/link.rs`, `win/workarea.rs` | the guard's end of the events; the work-area setter |
| `rf-shellguard/tests/tb_static.rs`, `real_link.rs`, `examples/taskbar_probe.rs` | source scans; the events against events the test creates; the probe |
| `rf-win/src/taskbar_hide.rs` | the host's thread, its hidden window (TaskbarCreated, display, work area, end of session, the 100 ms edge look), the real world, the panic net |
| `rf-win/src/dock/system.rs`, `dock/band.rs` | the dock's side of the edge (`take_edge`, `give_back_edge`, `appbar_held`) and the rule for a `TaskbarCreated` that is not a restart |
| `rf-app/src/shell/taskbar_hide.rs` | follows the setting (`decide`: start, turn off, the one-shot stale check) and the dock's band (`band_changed`); four one-line hooks in `dock/host.rs` |

## Tests

`cargo test --offline -p rf-shellguard` (the controller, the edge, the guard, the state file, the rules, the static scans, the events),
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
* `SPI_SETWORKAREA` (apply and restore), and the dock's side of the edge: `dock::system::take_edge` / `give_back_edge` against a real
  `DockSystem` (the registration removed and registered again, `LayoutChanged` and the islands moving to the monitor's bottom edge),
  `tbwin::wait_hidden`, the 100 ms edge timer of the host's thread.
* The tray host's route for the dock's own `SHAppBarMessage` calls (not hidden, tray host on), and the effect of `announcement_is_restart`
  against a real `TaskbarCreated` of the tray host.
* The guard's reading of the monitors and the taskbars before it puts the work areas back (`win/workarea.rs`: `EnumDisplayMonitors`,
  `GetMonitorInfoW`, the taskbar list, all on a per-monitor-aware thread), and the real `SPI_SETWORKAREA` with a rectangle that was worked
  out again after a resolution change or left out for a monitor that is gone. The rule itself (`tbwork::work_restore`) is tested.
* The release of the edge after an Explorer restart whose new taskbar cannot be hidden, with the real dock (`give_back_edge`
  registering the band again).
* Everything about Explorer's reaction: see the questions below.

## Questions only a run can answer (the lead's checklist)

1. Does `SW_HIDE` of the outer `Shell_TrayWnd` really remove the XAML taskbar from the screen (the island has its own composition), on
   both monitors, and does it stay hidden (how often does Explorer show it again: the once-a-second look is the answer)?
2. DETERMINED by the first run (2026-11-13): a hidden taskbar still reserves its strip. What is left to see: with the switch on, the work
   area is 2560x1362 (`dock_reserve` on, 78 px band) or 2560x1440 (off), the islands stand 10 px above the screen's bottom edge, and a
   maximised window ends exactly where the dock's band begins.
3. Where do Windows' own flyouts open (Start, Win+A, the clock, the volume) when their anchor is hidden? They are anchored by
   Explorer to the taskbar's position. The Windows key opens Windows' Start there unless `shell_win_key` is on; the switch's text
   says so and suggests turning that one on as well. Not determined: whether the flyouts are placed oddly or partly off-screen.
4. Does the notification area matter: with the taskbar hidden its icons are out of sight and the overflow cannot be opened. Without
   the tray host (shell plan D2, not built) the user has no way to reach them. Say so to the user who turns the switch on.
5. Win+D, Win+B, Win+T, full-screen applications, Aero Peek and hover previews of the hidden window.
6. Does `TaskbarCreated` arrive at a window that is not shown (it should: broadcast to top-level windows) and are the follow-ups at
   0.7 s / 3.2 s enough for the secondary taskbars?
7. DETERMINED: the band stayed above the empty strip (the strip was kept). Now: the dock sits on the bottom edge; see 2.
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
9. With the switch on (step 5) the work area is already the monitor less the dock's band: run the probe, maximise a window (it stops
   where the dock's band begins, no strip under the dock); switch off and see that the work area is back as it was (the probe shows it)
   and the dock is above the taskbar again. Also with `dock_reserve` off, and with the tray host switched on and off while hidden.
10. Optional, with your OK: restart Explorer from Task Manager (Windows Explorer > Restart) with the switch on: the new taskbar is
    hidden again within a few seconds.
11. **If anything is stuck:** `target\debug\rustyfiles-shellguard.exe --beside-guard 999999 --state-dir %TEMP%\rf-e1` (restores from the
    file and exits); or Task Manager > Windows Explorer > Restart (a new Explorer shows its taskbar).

Remember the cost: with the Windows taskbar hidden its notification icons are out of sight, and rustyfiles has no tray of its own yet.
