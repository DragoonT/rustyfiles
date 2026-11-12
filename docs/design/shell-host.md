# The shell host and the desktop cover (experimental)

Shell plan increments A1 (the host skeleton) and F4 (cover mode). Everything here is off unless asked for, and nothing here writes
the registry, sets the shell window, registers a hotkey or touches Explorer's windows.

## `rustyfiles.exe --shell`

The shell host is the dock's process under its new name: `--dock` is an alias and does exactly what it did. One process, one
single-instance group: a second `--shell` or `--dock` in the same Windows session finds the first one and leaves.

* **Mode.** At its start the host looks at who owns the shell window (`GetShellWindow()`, read-only). Explorer: `ShellMode::Beside`.
  Nobody, or somebody else (this very process included): `ShellMode::Shell`. A `--mode beside|shell` on the command line (the guard
  passes `--mode shell`) is recorded as what was *asked*; the look decides. In this increment `Shell` is only reported (log line,
  probe); nothing is done differently because of it.
* **Names per session.** The group's mutex is `Local\...` (per session already); its pipe is machine-wide, so the group's names now
  carry the session number from the second interactive session on (`rustyfiles-dock-...-s2`); the first one (the console of a single-user PC) keeps the
  name it always had, so that an older main app that is still running after an update still reaches (`stop`, `reload`, `ping`) a newer dock. The main
  app, DirectLook and the search pipe keep the names older copies know.
  NOT changed, listed as a finding: the main app's pipe (`rustyfiles-<user>`) is machine-wide too, so two sessions of one user name
  would reach each other's main app. It is an old behaviour of a different group.
* **`rustyfiles.exe --shell-probe`**: a windowless, read-only report in JSON: the mode (and what was requested), who owns the shell
  window and `Shell_TrayWnd` (program name and class only), the Winlogon `Shell` values (program names only) and the organisation's
  policy override, the monitors with their work areas, whether `rustyfiles-shellguard.exe` is next to the exe, the host's settings
  that are on. A release build has no console: `rustyfiles.exe --shell-probe > probe.json`.

## Cover mode (`shell_desktop_cover`, Settings, "Masaüstü" page; what the desktop does is in `desktop.md`)

With the switch on, the host puts OUR desktop (the view of `shell/desktop`) over Explorer's: one borderless window per monitor.
Explorer's desktop stays underneath and is visible again when the host stops.

| Question | Answer |
|---|---|
| Size | the monitor less ONE pixel row, the **bottom** one: a window that covers its monitor is taken for a full-screen application (Explorer's taskbar hides, our dock's full-screen test hides the islands). The usual taskbar sits on the bottom edge, and an auto-hiding one's strip is topmost there, so the missing row is behind one of them. (`rf_core::desktop::cover::geometry`) |
| z-order | directly above the shell's desktop window, below every other window, whatever is asked: `WM_WINDOWPOSCHANGING` rewrites `hwndInsertAfter` to the window that is above the desktop (insert *behind* it; "after Progman" would put the window under the desktop). Show Desktop raises the desktop above our windows: a WinEvent watch (foreground, minimize) makes the host check the order and put the windows back. The verdict for an ordinary (not topmost) window with only topmost windows above the desktop is `HWND_TOP`, not `HWND_NOTOPMOST`: Windows ignores NOTOPMOST on a window that is not topmost. While our own Show Desktop is on the windows are allowed to the top; it ends, as Explorer's does, when a program comes to the front (after a 1 s grace period). (`rf_core::desktop::cover::zorder`, `rf_win::desktop::cover`) |
| Taskbar, Alt+Tab | `WS_EX_TOOLWINDOW`, no `WS_EX_APPWINDOW`; no minimize box; Alt+F4 does nothing; left out of Aero Peek |
| Activation | activatable (the desktop takes the keyboard). A press on an ICON activates the window; a press on EMPTY SPACE does not (`MA_NOACTIVATE`), so it never takes the keyboard from the program the user was typing in. The press still clears the selection and starts the rubber band; if the band picked icons the window takes the foreground on mouse-up, so Delete / F2 / Ctrl+C act on them. (`rf_core::desktop::cover::activation`) |
| Monitors | one view per monitor, 1:1 with the monitor's pixels (no status strip, no scaling), each at its own DPI. Monitors added, removed or changed: windows created, closed or placed again after the display messages have been quiet for 400 ms (at most 2.5 s). (`plan`) |
| Layout | one layout store shared by the monitors' views (cells per monitor); a change of the sort, size or alignment on one monitor is drawn on all. First start on the real Desktop: Explorer's icon positions are imported once, before the first placement. The layout file is written from then on; if the import failed it is NOT written until the user arranges something (the sort order's placement of the first look is not their layout) and the import is tried again at the next start. An import that fitted 90 % or more of the positions is final: the rest stand where the sort order put them. |
| Grid | the import takes Explorer's own spacing with the positions (`IFolderView::GetSpacing`: 75 across and 99 down for medium icons on this machine, and the icons' positions step by the same numbers), so the cells are Explorer's cells and the icons stand in the same rows. A layout that never imports (a scratch folder; no Explorer to ask) starts on that measured Windows grid; a layout file written before the spacing was known keeps its square 75 pixel cells. Compared on the real Desktop, side by side with Explorer: the same rows, the picture 6 pixels under the cell's top, the label 3 under the picture and up to 71 pixels wide. Not the same yet: no elevation shield or shortcut overlays on the pictures, plain folder pictures where Explorer shows a preview of the content, and an icon whose Explorer row is under our dock moves to a free cell above it. |
| Writing | the user's own Desktop folder is WRITABLE (rename, delete to the Recycle Bin, paste, new, undo); Public and virtual items keep the view's protections. `RUSTYFILES_DESKTOP_DIR=<folder>` shows that folder on every monitor with a layout file of its own; a bad value fails closed to the real Desktop, READ-ONLY. It also fails closed when `RUSTYFILES_CONFIG_DIR` is set (a test or portable copy) and no scratch folder was given, or a `RUSTYFILES_DESKTOP...` variable other than `RUSTYFILES_DESKTOP_DIR` exists (a misspelt name): read-only, no import, a line on the desktop says so. `RUSTYFILES_COVER_REAL_DESKTOP=1` is the explicit "yes, the real Desktop" for such a copy. |
| Folders | a double click on a folder hands it to the main rustyfiles (`rustyfiles.exe <folder>`), the host being a side process |
| Glass | the cover windows are ordinary windows: they are in no capture exclusion list, so the glass above them bends them as it bends any window |
| Show Desktop / Minimize All | `rf_core::desktop::cover::showdesktop` (bookkeeping, tested) and `rf_win::desktop::showdesktop` (carried out over a trait, tested on a fake). Not bound to a key. |
| Stopping | the windows close. Nothing of Explorer's was hidden, moved or changed, so there is nothing to put back; `rf_win::desktop::cover` has a test that its code calls nothing that could change or message a window that is not ours, and every call that changes a window starts by checking that the window belongs to this process. |

## Keyboard layer (`shell_win_key`, `shell_own_switcher`)

Two more switches on the same Settings page, both off: a tap of the Windows key alone opens our Start panel, Alt+Tab opens our
switcher. They need one system-wide keyboard hook, which exists only in this process and only while one of them is on. Rules,
fail-open design and what was not run: `docs/design/shell-keys.md`. `RUSTYFILES_KEYS_LOG_ONLY=1` for a first try that decides and
logs but swallows, sends and opens nothing.

## Hiding Explorer's taskbar (`shell_hide_taskbar`)

A fourth switch on the same page, off: the host hides Explorer's taskbar windows while the dock runs, only with the guard
(`rustyfiles-shellguard.exe`, beside role) confirmed running, and shows them again on off, on quit, from a panic hook, and from the
guard if the host dies. Rules, the guard's protocol, the work-area question and the steps of a first try: `docs/design/taskbar-hide.md`.

## While it runs: Explorer leaving and coming back

Facts from the user's trial of 2026-11-12 (Explorer closed for two minutes with the development dock running): the dock, the Start panel
and the control centre kept working; (a) after Explorer came back the dock had lost the screen band it reserves (work area 2560x1314
before, 2560x1392 after) and never asked again; (b) nobody drew the wallpaper or the icons meanwhile; (c) a dock started just after the
restart wrote one log line and was gone; (d) the media keys did nothing unless Spotify's window was in front.

| Question | Answer |
|---|---|
| The band comes back | `rf_win::dock::band` is the registration as a state machine over the four toolbar calls (tested on a fake that models Explorer). `TaskbarCreated` (Explorer announcing a new taskbar) makes the next ask register again, once per announcement; not every announcement is an Explorer restart (our own tray host broadcasts it, so can any program), so when Explorer refuses the new registration because the old one still stands, the old one is given back and a new one taken. FOUND, probably the cause of "never asked again" (not reproducible without closing Explorer): the new taskbar sets the work area again AFTER our registration went through, from its own rectangle alone; the band we were granted was the same rectangle as before, so the old code saw "nothing changed" and sent no `ABM_SETPOS`. Now the work area is looked at too: when it does not leave the band free, the position is sent again, at most 3 times in a row (something that takes the band away on purpose is not fought for ever; a new announcement or a changed rectangle starts over). A registration that Explorer refuses right after its announcement is tried again 5 times, 1 s apart. |
| The mode is noticed while running | `rf_win::shellhost::watch`: no thread, no timer of its own. The dock's hidden window already ticks; every 2 s tick it calls `GetShellWindow()` once and compares the handle (nothing else happens while it is the same: no process is opened). A different handle, or the `TaskbarCreated` broadcast, makes ONE full look (`mode::detect`), never more than one in 2 s, announcements included (a change or announcement held back is picked up by the next tick; the band is registered again at every announcement regardless). The result is `SystemEvent::ShellChanged(mode)`; `shell::live` logs `shell host: mode beside -> shell` and tells the parts below. |
| Our desktop without Explorer | The window procedure already put the cover windows at the bottom of the z-order when there is no shell window (`zorder::target`), never activates them by a press on empty space, and the view draws the wallpaper itself (the cover is opaque), so the picture, its fit mode and the colour were already ours. What was missing, and is in `rf_core::desktop::cover::bare` + `rf_win::desktop::bare`: (1) the windows are placed again AT ONCE when the shell desktop window comes or goes (before, only the next foreground change did it: the windows left at the bottom would stay under Explorer's desktop when it came back, invisible); (2) the wallpaper is read from the registry alone while there is no shell desktop (the `IDesktopWallpaper` object may never answer without Explorer: test T3, unverified), by `wallpaper::current_via`; (3) the rule of what is drawn (picture + fit, or colour for a solid background, no picture, an unreadable file) as a tested pure function. Zero cost with cover mode off. |
| The work area without Explorer | `rf_win::shellhost::workarea`: with NO shell window the host sets the work area itself (`SPI_SETWORKAREA`, the monitor less its band, not written to the profile) and puts back what was there when it stops. Three locks, each tested: the keeper looks at the shell window again right before it acts and does nothing for Explorer (or any other shell); the real setter looks again before the call; and `shell::live` builds the keeper only with `RUSTYFILES_SET_WORKAREA=1` (no Settings row: it changes a system-wide setting). NOT RUN for real. |
| An exit always says why | `shell::exits`, one `shell host exits: <reason>` line (warn level, so the default log filter shows it) for: another host of the group answered (this used to be a silent `return` in `lib.rs`, the one path that fits "one line and gone"), a stop on the pipe, the session ending, no monitor, no island, all islands closed, a panic. A kill or a native crash cannot write anything. The module doc has the full table and the one candidate cause found for the trial's lost dock (a previous host still ending answers the new host's greeting, the new one leaves, the old one finishes ending). |
| Media and volume keys without Explorer | A key the focused window does not handle becomes `WM_APPCOMMAND`; "if a top level window does not process this message and instead calls DefWindowProc, DefWindowProc will call a shell hook with the hook code equal to HSHELL_APPCOMMAND" (documentation, read 2026-11-12), and a window registered with `RegisterShellHookWindow` receives it as the registered message `SHELLHOOK`. The shell is Explorer; with it away nobody takes the key. `rf_win::shellhost::appcmd` decides (next, previous, play/pause, stop to the system media session, the one the control centre's card uses; volume up/down/mute to the default output through `audio::set_volume` / `set_muted`, the slider's code, in 2 % steps like Windows' or the endpoint's own step; a held key steps from what it asked for, not from a reading that is behind). `shellhook` is the hidden window; `shell::live` makes it ONLY while nobody owns the shell window and drops it when a shell window is back. Beside Explorer (or any other shell) nothing is registered, and the decision answers "nothing" for every command anyway. No on-screen feedback (the dock has none for volume; the speaker symbol follows the endpoint by itself). The hook itself was not run. |

Keys that Explorer answers and that would also be dead without it (from how Windows behaves; not tried here): Win+E, Win+R, Win+D,
Win+M, Win+X, Win+number (taskbar buttons), Win+T, Win+B, Win+S / Win+Q, Win+A, Win+N, Win+Tab and Alt+Tab (the task switcher is a part
of the shell). The keyboard layer (`keylayer.rs`, off) covers two of them: a tap of the Windows key (our Start panel) and Alt+Tab (our
switcher). It states that Win+anything else is never touched, so Win+E / R / D and the rest are not covered, and are not built here.

## Known limits

* **One virtual desktop.** The cover windows belong to the virtual desktop they were made on; Windows hides a top-level window of another one while Explorer's own desktop window shows on all of them. On a second virtual desktop the user sees Explorer's desktop and icons, not ours (a rename or delete done on one is true on both, but the picture differs). Showing a window on every desktop needs an undocumented interface; not attempted.
* **Cost at rest is not measured.** Two swap chains of monitor size and the wallpaper pictures stay alive while the switch is on. The dock's own animations redraw only the dock's windows (`cover::refresh_dock_windows`), not the desktops. Working set and idle CPU with the switch on and off are for the live test to measure.
* **Show Desktop is not bound to a key** and its `Driver` keeps its own memory of what it minimized; when the covers come down because a program came to the front, the Driver still remembers (the next toggle restores what is still minimized and lowers an already lowered desktop, which does nothing). The binding increment decides whether that memory should end with it.

## Live test of a test copy

See the report of the increment; in short: `RUSTYFILES_CONFIG_DIR=<temp>`, `RUSTYFILES_MULTI_INSTANCE=1`, `RUSTYFILES_MUTE=1`,
`RUSTYFILES_DESKTOP_DIR=<scratch folder>`, `rustyfiles.exe --shell`, with `shell_desktop_cover = true` in the copy's settings.toml.
