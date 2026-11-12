# Start-up programs list and its dry run (`rf_win::startup`)

When rustyfiles is the Windows shell, Explorer does not run, so nobody starts the programs Windows starts at sign-in. This module
lists them, plans a run, and holds the (never yet run) code that would do the run. Beside Explorer only the list and the dry-run
plan are used: Explorer has already started everything.

## What is listed

In the order Windows handles them (research note 1, section 5.1):

1. `Policies\Explorer\Run` (HKLM, HKLM 32-bit view, HKCU)
2. `Run` (HKLM, HKLM 32-bit view, HKCU), with the user's on/off choice from `Explorer\StartupApproved\{Run, Run32}`
3. `RunOnce` (HKLM, HKLM 32-bit view, HKCU)
4. the user's Startup folder, then the common one, with `Explorer\StartupApproved\StartupFolder`

Per entry: source, name, the command as stored, program and arguments (cut as `CreateProcess` cuts a command line), on/off,
whether the program exists, whether a process with that image path runs now.

## What is not listed, and why

* **Packaged (Store) apps' start-up tasks.** There is no documented, read-only way to list them. Explorer uses an internal WinRT
  class; the state lives under `HKCU\Software\Classes\Local Settings\...\AppModel\SystemAppData\<package family>\<task id>` in a
  layout that was only observed (`State` 2 = enabled, 1 = disabled by the user). Reading that would be a guess about an
  undocumented layout, so they are left out, and a run cannot start them. Task Manager's Startup tab does show them, so its count is
  larger than ours by that many entries.
* `RunOnceEx`, group-policy start-up scripts, logon scripts and anything the Task Scheduler starts.

## What the plan does

* Shell-critical entries (`ctfmon.exe`) first, then a wait for the shell (limit 30 s), a settle time (2 s), then everything else in
  source order, 500 ms apart. `RunOnce` values named with `!` (delete after the command ran, waited for up to 60 s each) go last, so a
  hung installer delays nothing else. That is the one departure from source order.
* Skipped, with the reason: Safe Mode, HKLM RunOnce without an elevated shell, disabled by the user, an on/off value of unknown
  layout (never guessed), shortcut unreadable, no program, program missing, a registry command with an unexpanded `%VARIABLE%`, a
  shortcut that asks to run as administrator, already running (also: a process of the same file name whose path cannot be read, as
  an elevated instance is), same command as an earlier entry. A RunOnce job is never a duplicate: it has a value to delete.
* RunOnce: the value is deleted before the command starts (a failed delete means no start); with `!` after the command ran to its
  end. A skipped value is never deleted.
* Just before a launch the launcher is asked again whether the program runs (the list is read once, launches come seconds later).
* No UAC prompt, ever: an `.exe` is started with `CreateProcess`, which fails with `ERROR_ELEVATION_REQUIRED` for a program that
  asks for administrator rights instead of showing a consent dialog. Files that are not `.exe` go through `ShellExecuteEx` as
  Explorer does; that path cannot be shown to be free of a prompt (an association whose handler elevates).
* A stop request never splits a delete + launch pair; a wait for a `!` job ends on a stop request.
* `run_session` runs once per sign-in: it asks a marker (the logon session's LUID in a small file) and marks after a run that was not
  stopped, so a host the guard restarts does not start the whole list again.
* The real launcher and the real RunOnce delete cannot be built without a `ShellModeToken`, which `ShellModeToken::detect()` hands
  out only when no `explorer.exe` of the Windows folder runs in the session.

## To verify (only a second account or a VM can settle these; nothing here was guessed into the code)

1. Whether Explorer expands `%VARIABLES%` in a plain REG_SZ Run value. Until settled such an entry is skipped
   (`unexpanded-variable`), never started with a command cut at the wrong space.
2. Where Windows writes the on/off record of a machine-wide entry (HKLM Run, common Startup folder) when a **standard user** switches
   it off in Task Manager. We read HKLM only; on the development machine (an administrator) every record is there. Test: as a
   standard user disable an HKLM Run entry, then look for the value under `HKCU` and `HKLM` `...\StartupApproved\Run`.
3. The working folder Explorer gives a Run command (we pass none; a shortcut keeps its own).
4. Whether Explorer enforces the documented 260-character limit on a Run value (we split up to the 32,767 of `CreateProcess`).
5. Whether the 32-bit view of `Policies\Explorer\Run` is read by Explorer (the notes do not name it; it is listed, and empty here).
6. Whether the Startup folders and the policy keys are skipped in Safe Mode as Run is (documented for Run only).
7. The default delay Explorer applies (`Explorer\Serialize\StartupDelayInMSec`, absent here): our settle and stagger times are our own.
8. WOW64 file-system redirection is not reproduced.
9. Running with the real launcher: that a Run command starts, that `!` waits and deletes, that a program that needs administrator
   rights is reported and not prompted, that a second `run_session` in one sign-in starts nothing. Never run on the development PC.

## Probe

`cargo run -p rf-win --example startup_probe` prints counts per source and the dry-run plan with every entry named by its source and
number (`HKCU Run #3: exists, enabled, already running -> skip`); `-- --names` adds the names, for the user's own eyes. It opens the
registry for reading, lists the Startup folders, reads shortcuts on the shell thread and takes a process snapshot. It creates no
window and starts nothing.
