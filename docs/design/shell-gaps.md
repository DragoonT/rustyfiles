# The shell host's gaps without Explorer (from the user's two trials)

Facts from the trials of 2026-11-12 (the host ran as the only shell for minutes): the dock, the Start panel and the desktop worked;
minimized windows showed as small bars at the bottom left; entries of Start answered "Sınıf kaydedilmemiş (0x80040154)"; the rf-win tests
that use the shell's file engine never finished; an older process undid another writer's change of the settings; the glass fell back to
a capture that hid our windows from screen sharing; the Windows key opened nothing of ours. This page says what each part does now, what was
run and what was not. The command for the windowless probe is at the end.

## 1. Minimized windows (`rf_win::shellhost::minimized`, `rf-app` `shell/live.rs`)

With no window owning the shell window (`Shell::Nobody`: the same look the work area and the media keys use), Windows draws a minimized
window itself, as a small title bar at the bottom left. A shell stops that with `SPI_SETMINIMIZEDMETRICS` and the `ARW_HIDE` bit of
`MINIMIZEDMETRICS.iArrange` ("hide minimized windows by moving them off the visible area of the screen", documentation). The window stays
minimized and is restored by the same calls as always (`ShowWindowAsync(SW_RESTORE)`): the dock's click (`dock::launch::activate` /
`minimize`), the switcher and Show Desktop are unchanged, and the dock's task list does not look at a window's place. On this PC with Explorer
running the bit is set (the probe reads it), so this is the state every window is in beside Explorer.

* The host finds no shell window: the metrics are read, remembered, and the same metrics plus `ARW_HIDE` are set. Not written to the
  profile (no `SPIF_UPDATEINIFILE`), no broadcast (both calls pass `WRITE_FLAGS`, and a test checks that it holds neither bit).
* The host ends while there is still no shell: the remembered metrics are put back, only if they are still the ones we set.
* A shell window appears: NOTHING is written. Explorer sets this value itself when it starts; writing the old value back after it would show
  the bars again beside a running Explorer. This differs from the brief ("put it back exactly when a shell window appears again"): for
  Explorer the right state is Explorer's own. For another shell that is not Explorer the system default is put back, but only if the value is
  still exactly ours.
* Windows already minimized before the value was set keep the place they have until they are restored and minimized again (re-arranging
  them would move other programs' windows). A host that is KILLED with no shell leaves the value set until a shell sets it again.
* `RUSTYFILES_KEEP_MINIMIZED=1` leaves the metrics alone.
* Tested on a fake (the keeper, the two locks, the refusals); the real read is run by a test and the probe; the real write was NOT run.

## 2. Apps that did not start (`rf_win::activate`, `rf_win::apps::launch`, `dock::launch`, `rf-app` `shellless.rs`)

The failing path was `apps::launch`: the shell's own "open" of the entry's All-apps item (`shellfolder::open`, `ShellExecuteEx` with
`SEE_MASK_IDLIST` on `shell:AppsFolder\...`). Without Explorer it answers class not registered in 15 ms. Main already falls back (file behind
the entry, or the activation manager on a thread of its own with a 600 ms wait: without Explorer the activation manager STARTS the app and
never returns, measured). Now ONE shared function does the activation: `rf_win::activate::packaged(aumid, args)`; `apps.rs` and
`dock/launch.rs` (pinned and running packaged apps, jump-list tasks) both use it. With Explorer the order is today's: activation first, the
shell second, the caller waits for the activation's answer (an error in time is an error). An error after the 600 ms is logged as a warning, and for the dock (`Late::Shell`) the shell's open is then run from the activation's own thread, so the second step is not lost to a slow activation. Our process is tried as the manager's home only after a creation error or a server-trouble error (class not registered, RPC server unavailable, server died); an answer about the app (not installed) is final. With no Explorer each launch leaves one parked `rf-activate-app` thread until the process ends (a COM call cannot be cut short).

| Place | How it starts | Needs the shell's part of Explorer? | Now |
|---|---|---|---|
| Start panel, All apps entry | `apps::launch`: shell open of the item; then, by itself (`apps_direct`): the entry's Start-menu shortcut found by name and confirmed by its target (run by `ShellExecuteEx` on the file: arguments, start folder, show state, identity), else the program with the `System.Link.Arguments` the entry carries and its own folder, else `activate::packaged` | the first step yes (seen) | unchanged order; the failure text says what happened (below) |
| Dock pinned / running app, classic | `Launch::Link` / `Exe`: `ShellExecuteEx` "open" of the file with its folder and arguments | no (a file path); not run | unchanged |
| Dock pinned / running app, packaged | `activate::packaged`, then `shell:AppsFolder\...` open | the second step | activation on a thread of its own with the wait (before: the calling thread, which stays out without Explorer); when both fail the error says both; a late failure still reaches the shell step |
| Jump-list tasks | `jumplists/mod.rs` -> `dock::launch::start` / `start_packaged_with` | as above | as above |
| Start "Run as administrator" | `startapps::run_as_admin`: `runas` on the program, or on the `shell:AppsFolder` name for a packaged app | the packaged case yes | unchanged (UAC cannot be tried here); a failure now says the shell is missing |
| Desktop icons: This PC, Recycle Bin | our own views | no | unchanged |
| Desktop: Control Panel and other applets (`open_virtual`), items on a phone (`folder_view`) | `shellfolder::open` (shell item open) | yes for applets (they run in explorer.exe) | unchanged, listed |
| "Open with" | `shell::open_with`: `ShellExecuteEx` verb `openas` | unknown (shell32 in process); not run | unchanged, listed |
| Settings pages (`ms-settings:` links in the control centre, desktop menu) | `open_uri` -> `ShellExecute` protocol | unknown; not run | unchanged, listed |
| A folder we cannot list (`lib.rs`: `explorer.exe <arg>`) | spawns Explorer | it IS Explorer | CAUTION: with no shell, starting `explorer.exe` may bring up the Explorer shell itself. Not changed, not run; listed for the lead |

THE MESSAGE. `dock/start.rs` showed one text for every failure: "Başlatılamadı. Kaldırılmış ya da taşınmış olabilir." (may be removed or moved). It now
asks `shellless::start_failure_text`: class not registered with no shell window -> "the Windows shell (Explorer) is not running and Windows could
not open this entry without it"; with a shell -> "a component this entry needs is missing"; file / path not found (0x80070002, 0x80070003) -> the
old moved-or-removed text; the shell not answering in time -> says that; anything else -> "Başlatılamadı:" and the last reason in one line. Pure
and tested (`shellless::tests`).

Real starts that could be tried without disturbing the user: none through the file paths (every program, even a console command, shows a
window through `ShellExecuteEx`); the activation manager was asked for an id that is no app (the probe: it answers with an error in 53 ms and
starts nothing). Not run: any real start through the new shared function, with or without Explorer.

## 2c. File operations without Explorer (`rf_win::engine_guard`, `fileops.rs`, `recycle.rs`)

The trial's hang: `IFileOperation` (the shell's copy engine) with no `FOF_NO_UI`/`FOF_SILENT` opens its own progress window and undo record and never
ends without a shell. Each call is decided on the calling thread before anything is created (`engine_guard::plan`, pure):

| Operation (callers) | Shell window there | No shell window |
|---|---|---|
| Rename (`ops.rs`, `undo.rs`, desktop) `fileops::rename` | the engine | plain `MoveFileEx` without replace; the app's own undo list records it as before |
| Permanent delete (`fastdelete`, `status_center`) | our parallel delete, the rest to the engine | our parallel delete alone; an item already gone is done; what cannot be removed is an error that says so |
| Delete to the Recycle Bin (`status_center`, `organize`) | the engine (`FOFX_RECYCLEONDELETE`) | REFUSED, true message. No documented way to put a file into the bin without the shell: the bin is the shell's folder (`$Recycle.Bin`, `$I`/`$R` pairs); writing those by hand could leave the user's bin in a state Explorer does not read |
| Copy / move that goes to the engine (paste in the same folder, virtual folders, leftovers of our engine, `shell_copy`) | the engine | REFUSED, true message. Our copy engine does ordinary copies and moves itself and does not come here. LEFT: a same-folder paste ("x - Kopya") has no own way without the shell yet |
| Restore from the bin, undo of a delete (`recycle::restore`, `undo.rs`), delete inside the bin (`delete_by_keys`) | the bin's verbs | REFUSED, true message |
| New folder, new text file, bulk rename | plain file calls | unchanged (they never used the engine) |
| Empty the bin (`recycle::empty`, `SHEmptyRecycleBinW` with all three UI flags) | the shell function | not changed: the documentation does not say it needs a shell; not run (it would empty the user's bin) |

A TIME LIMIT on every call that goes to the engine: it runs on a thread of its own (not `sta::call`: a hung job there would hold the one shared
shell thread, and with it every menu); the caller waits for a first sign of life from the engine's progress sink (`StartOperations` or any item
callback). None within 60 s (`engine_guard::START_LIMIT`) -> `EngineStalled`, a true message ("did not start; Explorer may have closed or hung;
it may still finish"). After the first sign there is no limit: a big copy is legitimately long, the Status Center can cancel, and a conflict
prompt waits for the user. LEFT: the bin's own verbs (restore, delete in the bin) have no sign of life to wait for, so they are gated but not
time limited. The Status Center shows the Turkish sentences from `shellless::op_failure_text`.

PARTLY DONE. Our own copy engine runs first and hands only its leftovers (`fastcopy::finish`: items it left whole, single files that failed) to the shell's
engine. With no shell that hand-over is refused; the refusal used to end `finish` at once with "Nothing was changed" although the copies were at the destination
and, for a move, the sources were not removed. Now the refusals are counted, the check and the removal of the VERIFIED sources still happen, and the error is
`engine_guard::PartlyDone` ("N items could not be copied/moved because that needs the Windows shell ... Everything else was done"; the Status Center's Turkish
sentence says the same with the count). A permanent delete that removed some items and could not remove others is `PartlyDone` too (it said "needs the shell" before).
`NoShellEngine` keeps the "nothing was changed" sentence only for a refusal made before anything was done. Tested: `fastcopy::tests` (copy and move with leftovers, no shell).

TESTS: a test that needs the shell (`fileops::tests::unique`, `shell_window::tests`) calls `engine_guard::not_run_without_shell(name)` first; without
a shell window it prints `NOT RUN: <name> ... no shell window exists` and returns, so the suite ends. `engine_guard::with_no_shell` makes the
calling thread believe there is no shell window, so the no-shell paths run for real on temp files while Explorer runs (tests and the probe).

## 2d. The settings file and another writer (`rf-app` `settings_merge.rs`, `Settings::load` / `save`)

A process remembers what the file held when it read it (its baseline, the same table form a save produces, taken before the migration). A save:
works out the keys THIS process changed (at any depth of nested tables, plus keys that are no longer written); if none, writes nothing (a process
that changed nothing never writes, on exit or otherwise); otherwise reads the file as it is now, under a mutex shared by the session's processes, puts
those changes on top of it, and writes by temp file and rename. Keys it does not know stay (a newer build's setting survives an older build's save).

* a process that FOLLOWS the file (the dock reloads it when another process writes): `Settings::load_followed` gives the table the file held, and the dock calls
  `Settings::follow_file` with it when it replaces its settings, so its baseline moves too. Before, the dock's baseline stayed at its start-up table: a value another
  writer had set since looked like the dock's own change and its next save (a size slider) wrote it back over whatever the file had become meanwhile.
* two writers, different keys: both survive. The same key: the last to save wins (per key of a nested table).
* a value set back to its default is not written by the settings struct: it is removed from the file, nothing else is.
* a missing file: the whole of the process's settings are written. A broken file (not UTF-8, not TOML) when the process has changes: set aside as
  `settings.toml.bozuk-<time>` and the whole written, so the user's changes are not lost and the broken file is kept. A file that cannot be read now
  (locked): nothing is written and the baseline stays, so the next save carries the same changes (a save at exit has no next one: the price).
* the file is now written with its keys in alphabetical order (the TOML table type here is ordered by key; the struct order is gone). Cosmetic.
* Tests: the rule on tables, the real `Settings` (a desktop switch from one writer, a session from another, a future key), and eight threads saving
  25 times each into one temp file.

## 2e. Our windows in screenshots without Explorer (`rf_win::glass`)

With no Explorer `CreateForMonitor` failed ("Could not capture the given monitor"); `choose_backend` took Desktop Duplication for the whole run
and `WDA_EXCLUDEFROMCAPTURE` hid every glass window from screenshots, recordings and screen sharing. Built (`glass/retry.rs`, hooks in `engine.rs`): a
fallback that came from a SESSION failure with the permission present (not from the system saying no, not from the permission being withdrawn) is tried
again: after 10 s, 30 s, 60 s, then every 5 minutes, only while some glass is in view, the engine reads the permission and tries a session; when one opens
it goes back to sessions for all its windows (flag removed from the glass windows and the menus/flyouts, every window drawn from a new backdrop).
Cost: nothing while sessions are in use; in the fallback one comparison of two instants per turn, and a permission read (about 3 ms) plus a session
attempt on each due try. The schedule is tested (`retry::tests`); the engine hook is NOT run (it needs a failing machine).

LISTED, not built: (a) another way to get a capture item: `CreateForWindow` on the desktop window is not documented to work and was not tried; the
cause of the failing `CreateForMonitor` without Explorer is unknown (the error text only). (b) In the fallback, "only the glass windows themselves out": the
flag is per window, so a glass file window is hidden whole; the desktop cover is in no list in either mode (it is not glass). Menus and flyouts registered
as "extras" are hidden in the fallback too. There is no per-region exclusion in Desktop Duplication, so a glass window cannot be shown to screen
sharing and kept out of its own backdrop at the same time.

## 3. The Windows key (`rf_win::winkey`, `shell/keylayer.rs`, setting `shell_win_key`)

Read whole with `shell-keys.md`. Found and fixed:

* After the layer gave up (five reinstalls in a minute) it stayed off until the settings were switched off and on: a shell host with no Explorer has no
  other Windows key. Now it tries the hook again after 5 minutes (`GIVE_UP_COOLDOWN_MS`), from scratch, and gives up again if it is lost again.
  Tested with the manual clock.
* The log said "layer on" / "layer off, hook removed" with no reason. Now: `keys: ready - a tap of the Windows key opens our Start panel; ...` once when the
  layer is up and again when what the keys do changes (Explorer came back), and `keys: layer off, hook removed (both settings are off | the host is ending |
  the service was dropped)`.
* The row's text on the Dock page now says that with Explorer not the shell the Windows key opens nothing, so this switch is NEEDED for it.
* Checked and found right: a key reported stuck (the layer never swallows a Windows key, Alt or a modifier; the mask key is one `SendInput` of down+up),
  Start opening after Win+something (any other key between makes the hold a chord, tested), both Windows keys (not a tap), key repeat (one tap),
  a hold seen only while the layer stood down (never a tap).
* Found by the review and fixed: Win+E pressed twice in one Windows-key hold (the release of the taken E never reached the machine, so the second press looked like
  an auto-repeat and went to the focused program); a key-up a program sent (a remote-input tool, an IME) ended the tracking of a taken key while the real key was
  still held; and a Windows key believed down after a release the hook never saw made a typed `e` or `d` a chord. The layer now asks the keyboard's own state
  (`GetAsyncKeyState`, through the same `KeyProbe` the heartbeat uses) once, at the press of E or D with a Windows key believed down, and drops the belief when no
  Windows key is down. Each has a layer test that fails without the fix. NOT run: that `GetAsyncKeyState` called inside the hook shows a Windows key whose press the hook let through as down (it should: the hook passed it on; if it did not, the chord would never be taken and the live test shows it as Win+E doing nothing).
* Still open, written down: a tap while the hook was silently dropped is not seen (the check needs a key held for half a second; no canary key is sent on
  purpose); Win+mouse (no mouse hook); the secure desktop and Win+L (the second beat installs the hook again, counts toward the limit of five);
  an elevated window in front (UIPI: the hook gets no keys); a remote session (events may arrive flagged as injected, which the layer ignores, so it fails
  to Windows' behaviour); full-screen games (the layer stands down, tested). None of these was run.

### Win+E and Win+D (behind the same switch, only while Explorer is NOT the shell)

`WinMachine` reports a chord of ours when a Windows key (one of them) is down alone, no modifier was held at its press, no other key came before, and `E` or `D`
goes down. The layer then swallows that press, its auto-repeats and its release (also if it stood down meanwhile), sends ONE mask key for the hold (so
Windows' own Start does not open on the Windows key's release), and posts `WinCombo`. The host opens a rustyfiles window (`shell:MyComputerFolder`, which we show
ourselves) or toggles our Show Desktop (`rf_win::desktop::showdesktop::Driver`, kept for the process so it remembers what it minimized). The Windows key
itself is still never swallowed. Beside Explorer (`live::current() == Beside`) the keys are left to it (`keylayer::combos_wanted`); the host follows the shell window
(`live::on_shell_changed` -> `keylayer::shell_changed`). `RUSTYFILES_KEYS_LOG_ONLY=1` posts and logs `win+e: would open a file window` and swallows nothing.
Tests: machine, layer (swallow, repeat, release after stand-down, one mask, log-only, shifted chords, injected keys), queue, service, host handler. The hook
itself and the real mask, `SendInput` and window opening were NOT run.

### Keys Explorer answers (from how Windows behaves, not tried here) and what the layer does with them

| Key | Dead without Explorer | The layer today | Could be handed to |
|---|---|---|---|
| Win (tap) | Start | opens our Start panel | - |
| Win+E | file window | opens a rustyfiles window | - |
| Win+D | show desktop | our Show Desktop | - |
| Alt+Tab | task switcher | our switcher, `shell_own_switcher` | - |
| Win+R | Run dialog | no | `rf_win::runcmd` / the quick search |
| Win+M, Win+Shift+M | minimize all / undo | no | `showdesktop::Driver::minimize_all` / `undo_minimize_all` (exist) |
| Win+Tab, Win+Ctrl+D... | task view, virtual desktops | no | our switcher (all windows); virtual desktops need Windows' undocumented interface |
| Win+S, Win+Q | search | no | the quick search (`open_quick_search`) |
| Win+A, Win+N, Win+B, Win+T, Win+number | control centre, notifications, tray, taskbar buttons | no | our panels and dock apps |
| Win+X | quick menu | no | the dock's app menu (new) |
| Win+L | lock | Windows' own (winlogon), not Explorer's | - |
| Win+arrows, Win+Space, Win+PrtScn, Win+Shift+S, Win+V, Win+P, Win+I | snap, input language, screenshots, snipping, clipboard history, projection, settings | each is a system component or a shell part: which of them need Explorer was not checked | not built |

## What was run and what was not

Run: the whole rf-win and rf-app test suites (see the report), clippy, the probe below, the parts above marked tested. Not run (the lead does it): the real
write of the minimized-windows metrics, any real start through `activate::packaged`, the shell engine with no shell, the glass's return to sessions, the
keyboard hook with Win+E / Win+D, the Win+mouse and secure-desktop cases, and everything in "Still open".

## The probe

`cargo run --offline -p rf-win --example shell_gaps_probe` (windowless, read-only except for temp files it creates and removes): prints whether a shell window
exists, the minimized-windows metrics, the plan of each file operation with and without a shell, the no-shell paths run for real on temp files, and the
activation manager's answer for an id that is no app.
