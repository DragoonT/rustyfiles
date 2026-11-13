# The shell host's keyboard layer (experimental, off by default)

Shell plan increment B7 and the switcher's wiring (rows C5 and D9 of the plan's section 1.2). Two settings, each ONE switch on the
dock's Settings page ("Dock", DENEYSEL), both off:

| Setting | What it does |
|---|---|
| `shell_win_key` | A tap of the Windows key alone opens OUR Start panel instead of Windows' Start. |
| `shell_own_switcher` | Alt+Tab opens OUR window switcher (`shell::switcher::open`, `hooked = true`) instead of Windows'. |

The honest line each switch shows (Turkish with an English pair): experimental; rustyfiles then listens to the keyboard with a
system-wide hook; it looks only at the Windows key (or at Alt+Tab and the keys of an open switcher); it stores, records and forwards
no key; it sends one invisible do-nothing key and no other; when rustyfiles stops or stalls Windows' own behaviour is back.

Nothing here registers a hotkey, hides Explorer's taskbar, binds Win+D, or writes the registry. The sweep tool for
`RegisterHotKey` chords is not part of this increment.

## Where the pieces are

| File | Role |
|---|---|
| `rf-win/src/switcher/hook.rs` | THE hook: `KeyFeed`, `LowLevelHook`, `FakeFeed`. Extended, not duplicated: `start_raw` delivers every event as a `RawKey`, `RawTracker` keeps Shift and Ctrl, `start` (the chord's view) is made from it. |
| `rf-win/src/switcher/chord.rs` | The Alt+Tab state machine, unchanged in behaviour; its list of swallowed keys is now a bit set (`Copy`, no allocation). |
| `rf-win/src/winkey/machine.rs` | The Windows-key machine (pure). |
| `rf-win/src/winkey/layer.rs` | The hook CALLBACK: one `on_event` that runs both machines and applies the rules below. |
| `rf-win/src/winkey/shared.rs` | The atomics the callback shares with the interface thread. |
| `rf-win/src/winkey/queue.rs` | The ring through which the callback posts. |
| `rf-win/src/winkey/mask.rs` | The mask key: the ONE `SendInput`. |
| `rf-win/src/winkey/service.rs` | Install, remove, heartbeat, dropped-hook detection, the dispatcher thread. |
| `rf-app/src/shell/keylayer.rs` | The host's side: settings, tasks, `handle` (what a post does), the real actions. |
| `rf-app/src/dock/host.rs` | Three one-line hooks: `keylayer::on_settings` at start and at each settings reload, `keylayer::stop` in `shutdown`. |

## When the hook exists

Only inside the shell host process (`rustyfiles.exe --shell` / `--dock`), and only while at least one setting is on. `KeyService::
set_wanted(win, switcher)` installs it when the first goes on and removes everything (hook, dispatcher thread, shared state) when both
are off. It is also removed by `keylayer::stop` at the host's shutdown and by `Drop` of the service. A process that is killed loses
its hooks with it. The beat task (every 250 ms) and the posts task exist only while the layer does.

## The callback: what it may and may not do

`KeyLayer::on_event` runs on the hook thread, inside Windows' input path. It reads and writes atomics and its own `Copy` fields,
posts one `u32` to a ring, and, for the mask key, calls `SendInput` (which does not wait for anyone). It takes NO lock the interface
thread could hold (the chord is owned by the callback alone; the interface talks to it through an atomic "reset" request), allocates
nothing (`Chord` and `WinMachine` are `Copy`; a test pins it), logs nothing, touches no COM. A panic inside it is caught: the key
passes and the hook stays. An event that the callback's own `SendInput` makes the system deliver again is passed untouched.

Tests that scan the sources (`winkey::scans`): the only `SendInput` is the mask key's (one call, one `wVk`, `MASK_VK`); no key code
(`vk`, `scan`, `flags`, key types) is formatted into a log line; the callback files contain no `log::`, no `Vec`, `String`,
`format!`, `Box::new`, `Arc::new`, `Mutex`, `.lock()`, `thread::sleep`, `join`, COM; nothing but `hook.rs` calls
`SetWindowsHookEx`; no test starts the real hook.

## The Windows key

`WinMachine`: the Windows key goes down and comes up with no other key between, no Shift/Ctrl/Alt held when it went down. Left and
right keys, auto-repeat of a held key (one tap), both keys together (not a tap), a key already down when the hook came up (its
release is not a tap), injected events (ignored: the dock's own Start button sends the Windows key this way when its own-Start
setting is off, and the mask key comes back this way). On the release of a clean tap the callback sends the mask key, lets the
release pass, and posts `StartTap`; the host toggles the Start panel (`dock::toggle_start`). Win+anything is never touched: the other
key makes the hold a chord, the Windows key's own events are never swallowed.

While the layer does not look (stale beat, a full-screen program, the setting off) the machine still learns which Windows keys are
held (`WinMachine::observe_unseen`) and that their company was not seen: such a hold is never a tap, and the first auto-repeat after the
layer is back is not a fresh press (Win+arrow held through a stand-down must not end as a tap).

When our panel cannot open after the mask was sent (a full-screen program that came up less than a beat ago, the dock being rebuilt,
no glass to sit above) `dock::toggle_start` answers false, the host logs `start: our panel could not be opened; Windows' Start was
asked for` and sends the dock's own "start menu" tap (`rf_win::dock::triggers::start_menu`, an injected Windows key, which the layer
ignores), so the tap is not lost. This is the one send besides the mask key, and it is the dock's existing helper, not new code.

Not seen: a MOUSE action between the press and the release (Win+click, Win+wheel). The keyboard hook does not get mouse events, so a
Win+wheel that ends in a lone release counts as a tap. Rare; a live check item.

## Alt+Tab

The existing chord (`switcher/chord.rs`): Alt held, Tab opens (all windows) and steps, Shift+Tab backwards, the key above Tab (by scan
code) opens/steps within the app, arrows move, Enter chooses, Escape cancels, releasing Alt chooses. The callback runs the chord and
posts its effects; the host's `handle` turns them into `switcher::open` (once, for `Open`), `apply_effect` (the rest). `on_closed`
of the open request tells the chord when the switcher closes by itself (`Shared::chord_closed(seq)`); an opening is numbered (`seq`)
so that a late "closed" of an earlier opening cannot end a newer one.

AltGr (a right Alt with a made-up Ctrl) and Ctrl+Alt+Tab are not the chord: the hook counts a key PRESSED with Ctrl down as another key.
A RELEASE is always the key it is (`switcher::hook::translate`): the chord finds the release of a swallowed press by the key's own
identity, so Ctrl pressed between a swallowed Tab and its release cannot make the release pass and leave a stale mark that would swallow
the release of a later, plain Tab.

## The rules, and the test that pins each (rf-win `winkey::layer::tests` unless noted)

| Rule | Test |
|---|---|
| FAIL OPEN, stale heartbeat | `stale_heartbeat_fails_open_every_key_passes_and_nothing_is_swallowed_sent_or_posted` |
| FAIL OPEN, heartbeat never made | `a_heartbeat_that_was_never_stamped_passes_every_key` |
| Back when the beat returns | `the_layer_comes_back_when_the_heartbeat_does` |
| Stand down for a full-screen program | `a_full_screen_program_in_front_stands_both_layers_down` |
| A swallowed press has its release swallowed, even after standing down | `a_key_that_was_swallowed_down_has_its_release_swallowed_even_after_the_layer_stood_down` |
| A press that passed never has its release swallowed | `a_key_that_passed_down_has_its_release_passed_not_swallowed`, `a_press_while_the_layer_stands_down_passes_and_its_release_passes_after_the_layer_comes_back` |
| Alt and the Windows keys are never swallowed | `alt_and_the_windows_keys_are_never_swallowed_in_any_state_of_the_chord`, `a_clean_tap_of_the_windows_key_sends_the_mask_once_...` |
| Win+anything untouched | `win_plus_anything_is_never_touched_nothing_swallowed_nothing_sent_nothing_posted` |
| The switcher could not be opened: the chord is given up at once | `when_the_switcher_could_not_be_opened_the_chord_is_given_up_at_once` (layer), `a_switcher_that_could_not_be_opened_gives_the_chord_up_at_once` and `when_the_switcher_cannot_open_the_next_tab_goes_to_windows_through_the_whole_path` (rf-app `keylayer`) |
| The switcher closed by itself | `a_switcher_that_closes_by_itself_ends_the_chord_and_the_next_tab_opens_a_new_one`, `a_late_close_of_an_older_opening_does_not_end_the_newer_one` |
| Setting switched off in the middle of a chord | `switching_the_setting_off_in_the_middle_of_a_chord_closes_it_tells_the_interface_and_passes_new_keys` |
| Injected events ignored | `injected_windows_key_events_are_ignored_...`, `injected_alt_tab_is_ignored`, machine `events_a_program_sent_are_ignored_whatever_they_are` |
| A key already down when the hook came up | `a_windows_key_that_was_already_down_when_the_hook_came_up_does_not_tap_on_its_release` |
| Log-only mode | `log_only_decides_and_posts_but_swallows_nothing_and_sends_nothing` (layer), `in_log_only_mode_nothing_is_done_and_the_lines_say_would` (keylayer) |
| One SendInput, the mask key's | `winkey::scans::the_only_send_input_in_the_keyboard_layer_is_the_mask_keys` |
| No key code in a log line | `winkey::scans::no_key_code_is_formatted_into_a_log_line`, rf-app `keylayer::no_log_line_names_a_key` |
| A swallowed press's release is swallowed even when Ctrl went down in between; the next plain press and release pass | `a_swallowed_keys_release_is_swallowed_also_when_ctrl_went_down_in_between_and_the_next_plain_press_and_release_pass` |
| A Windows key held across a stand-down is never a tap | `a_windows_key_held_across_a_stand_down_is_not_a_tap_when_it_is_released_after_the_layer_is_back`, `a_windows_key_pressed_while_the_layer_stands_down_and_released_after_it_is_back_is_not_a_tap`, machine `a_hold_seen_only_while_the_layer_stood_down_is_never_a_tap_and_its_auto_repeat_does_not_arm_the_machine` |
| A reset from the interface is never lost | `shared::a_reset_written_while_another_is_being_taken_is_never_lost` |
| Cycling does not reinstall the hook (Tab is not compared) | `service::a_tab_the_hook_swallowed_is_not_compared_with_the_keyboard_so_cycling_does_not_reinstall_the_hook` |
| A chord left open with Alt up is closed | `service::a_chord_left_open_with_alt_up_on_the_keyboard_is_closed_after_two_beats_and_stops_swallowing`, `a_chord_with_alt_really_held_is_never_closed_by_the_beats` |
| A tap whose panel cannot open is not lost | rf-app `keylayer::a_tap_whose_panel_could_not_open_asks_windows_for_its_start_and_does_not_say_toggled` |
| A hook thread that starts late is not left running | `hook::a_hook_thread_that_comes_up_after_the_starter_gave_up_ends_itself`, `a_hook_thread_that_is_already_up_when_the_starter_gives_up_is_told_to_quit` (the handshake only; the thread itself is not run) |
| No allocation, lock, wait, logging in the callback | `winkey::scans::the_hook_callback_files_...` (two tests), `chord::the_chord_owns_no_heap_block_...` |

## Fail open: the heartbeat

The interface thread calls `KeyService::heartbeat` every 250 ms (`HEARTBEAT_EVERY_MS`), from a task on the foreground executor, so a
beat proves the thread is processing events. It stamps the time and hands over "a full-screen program is in front" (the dock's own
test, `dock::is_fullscreen`). The callback compares the stamp with its clock on every event: older than 1000 ms (`STALE_AFTER_MS`,
four missed beats), or never stamped, means STAND DOWN: every key passes, nothing new is swallowed, sent or posted. The first beat
is made when the hook is installed. Releases of keys that were swallowed before still follow their presses (rule above), so a stall
in the middle of Alt+Tab cannot leave a key down.

When the layer stands down in the middle of a chord it closes the chord and posts `Lost`; the host closes the switcher.

### Why stand down for a full-screen program

Our panels do not open above a full-screen window (`open_panel` returns at once, the islands are hidden), so a swallowed Windows-key
tap would do nothing at all, and a game that is asked to leave full screen by Alt+Tab would lose its own. Windows' own behaviour is the
better one there. The test is the dock's, stamped with the beat, so it can be up to 250 ms late.

## A hook that Windows dropped

Windows removes a low-level hook without a word when its callback is too slow. Ours never waits, so the usual causes are a stall of the
whole process (suspend, sleep/resume, starvation) or a bug. `KeyService::heartbeat` looks for it three ways, none of which sends a key:

1. the feed says it is not running (the hook thread ended, or the hook could not be installed);
2. a STALL: this beat came more than 3 s after the previous one;
3. a DISAGREEMENT THAT LASTS: the layer keeps a belief about the Windows keys and Alt (never swallowed; injected ones count), and each
   beat reads the keyboard's own state of them with `GetAsyncKeyState`; two beats in a row that disagree (0.5 s) mean the hook is no
   longer called. Tab is NOT compared: whether `GetAsyncKeyState` shows a key a low-level hook swallowed is not known (see "not run"),
   and if it does, holding Tab while cycling would reinstall the hook in the middle of the chord.

Win+L (and UAC, Ctrl+Alt+Del): the Windows key comes up on the secure desktop and the hook never sees the release, so the belief stays
"down" while the keyboard says "up": the second beat reinstalls the hook. That is wanted (the fresh layer starts from the keyboard's
state, so the first Windows-key tap after unlocking is not lost to a key the old layer thought was held), and the log says
"installing the hook again". It counts toward the limit of 5 within a minute, which a person locking the screen does not reach.

A separate check at the same beats: a chord open as a HELD one (the layer thinks Alt is down) while the keyboard says Alt is up for two
beats in a row is ended at once (`Shared::close_current_chord`) and `Lost` is delivered, so a switcher nobody is steering does not keep
swallowing Tab, the arrows, Enter and Escape. The case: Alt came up where the hook cannot see (see "Known limits", elevated windows).

Then the hook is installed again from scratch (fresh layer seeded from the keyboard, `Lost` posted). Limits: not twice within 2 s;
after 5 installations within a minute the layer gives up and stays off until the settings are switched off and on.

Honest gap: check 3 can only see a key that is held for half a second. A hook dropped for no visible reason is found the first time
Alt or the Windows key is held that long; until then the keys do what Windows does, which is the safe side. No canary keystroke is
injected on purpose: it would reset the idle timers (screen saver, lock, sleep).

## The mask key

Windows opens Start (and lights the menu bar) when the Windows key or Alt goes down and up with nothing between. The layer sends
virtual key 0xE8 (down and up, one `SendInput` call, `dwExtraInfo` tag "RFKY") from inside the callback before the release is let
through; the release is being processed and not yet committed, so the mask lands before it (the method AutoHotkey recommends).
NOT RUN HERE. If the first live test shows both Starts opening, the mask is landing after the release; the alternative is to swallow
the real release and send "E8 down, E8 up, Win up" in one call, which this increment does NOT do because a refused injection
(UIPI, an elevated window in front) would leave the Windows key stuck down. A mask the system refuses is counted and logged
("the system did not take the mask key"); Windows' Start then opens as well.

## Log-only mode

`RUSTYFILES_KEYS_LOG_ONLY=1` (exactly "1"): the layer runs its machines and posts its decisions, swallows nothing, sends no mask key,
and the host opens nothing: the log says `start: would toggle (log only)`, `switcher: would open (log only)`, `switcher: would choose
(log only)`. The first thing to try.

## Log lines (decisions only)

`keys: layer on (windows key: on, switcher: on, log only: no)`, `keys: hook installed`, `keys: installing the hook again (...)`,
`keys: layer off, hook removed`, `start: toggled`, `switcher: opened`, `switcher: could not be opened; the chord was given up`,
`switcher: chose` / `cancelled`, `switcher: closed (the keyboard layer lost track of the chord)`. Never a key.

## Win+E and Win+D, and the pause after giving up (added with `shell-gaps.md`)

With `shell_win_key` on and Explorer NOT the shell, Win+E opens a rustyfiles window and Win+D toggles our Show Desktop: the layer swallows the press of E or D (and its
repeats and release), sends one mask key for the hold and posts `WinCombo`. The Windows key itself is still never swallowed; any other key, a modifier, both Windows keys
or an injected key make the hold an ordinary one. Beside Explorer the keys are left to it. After the layer gave up (five reinstalls in a minute) it tries the hook again
after five minutes. The log has `keys: ready - ...` when the layer is up or its state changes and the reason in `keys: layer off, hook removed (...)`. The table of the
keys Explorer answers is in `shell-gaps.md`.

WIN+SHIFT+S (our screen snip, `snip.md`) is the one chord with a modifier. `WinMachine::feed_ex` takes it when the Windows key was held alone (a Shift may come before it or after it; Ctrl or Alt
anywhere in the hold makes it Windows' again), S is pressed with Shift down and nothing else, and the hold is clean. The layer then swallows S (and its repeats and release), sends the one mask key and posts
`WinCombo(Snip)`; the Shift and the Windows key pass as always. Win+S alone, Win+Shift+E and Win+Ctrl+Shift+S are untouched. Beside Explorer the chord is Explorer's.

## Known limits

* Secure-desktop input (UAC, Ctrl+Alt+Del, Win+L) never reaches the hook: Windows' own, as it should be.
* An elevated window in front: a hook of a normal-integrity process is NOT given the keys typed into a window of a higher integrity
  level (UIPI; the widely documented behaviour, NOT verified here). So Alt+Tab and the Windows key keep doing what Windows does there,
  and the mask key may be refused as well. A mixed sequence (Alt held in an elevated window, Tab seen after the focus moved, Alt
  released unseen) can open the chord as a held one with no release to come; the second check above ends it after about half a second.
  Escape or a click outside also ends it, as with any switcher.
* The Start panel is brought to the front by the quiet method (`bring_to_front_quietly`). After a Windows-key tap, with no Alt held,
  Windows may refuse the foreground to a process that was not clicked; then the panel is shown without the keyboard and closes itself
  like any panel that never got the foreground. Not run here; a live check item.
* `dock_own_start = false` makes the dock's logo send the Windows key; that key is injected, so the layer ignores it and Windows'
  Start opens, as that setting asks.
* Remote desktop, virtual machines and games that read the keyboard directly are not tested.

## What was NOT run (the real paths)

Assumptions that nothing here could check, with what the design does about each:

* Whether `GetAsyncKeyState` shows a key that a low-level hook swallowed. Not relied on: Tab is not compared. Live step: hold Alt+Tab
  for 2 seconds and look for "installing the hook again" in the log (there must be none).
* Whether a hook of a normal process gets no keys while an elevated window is in front (UIPI). Not relied on beyond the second check.
* The Win+L reinstall (above): expected, not seen.
* `windows_start` (the fallback when our panel cannot open): the injected tap reaching Windows' Start after the mask. Hard to provoke.

The real hook (`LowLevelHook::start_raw`, `SetWindowsHookExW`), the real `SendInput`, the real `GetAsyncKeyState` probe, the GPUI
wiring (`GpuiActions`, the tasks, the settings switches on screen), the Start panel and the switcher opened from a key, the mask
key's ordering against the release, the re-entry of the callback through its own `SendInput`, the foreground handover after a tap.
Everything else ran through `FakeFeed`, fakes of the mask and the probe, and a fake of the host's actions. See the increment report
for the numbered live steps.
