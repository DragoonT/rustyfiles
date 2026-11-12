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

## Cover mode (`shell_desktop_cover`, Settings, "Dock (deneysel)")

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
| Writing | the user's own Desktop folder is WRITABLE (rename, delete to the Recycle Bin, paste, new, undo); Public and virtual items keep the view's protections. `RUSTYFILES_DESKTOP_DIR=<folder>` shows that folder on every monitor with a layout file of its own; a bad value fails closed to the real Desktop, READ-ONLY. It also fails closed when `RUSTYFILES_CONFIG_DIR` is set (a test or portable copy) and no scratch folder was given, or a `RUSTYFILES_DESKTOP...` variable other than `RUSTYFILES_DESKTOP_DIR` exists (a misspelt name): read-only, no import, a line on the desktop says so. `RUSTYFILES_COVER_REAL_DESKTOP=1` is the explicit "yes, the real Desktop" for such a copy. |
| Folders | a double click on a folder hands it to the main rustyfiles (`rustyfiles.exe <folder>`), the host being a side process |
| Glass | the cover windows are ordinary windows: they are in no capture exclusion list, so the glass above them bends them as it bends any window |
| Show Desktop / Minimize All | `rf_core::desktop::cover::showdesktop` (bookkeeping, tested) and `rf_win::desktop::showdesktop` (carried out over a trait, tested on a fake). Not bound to a key. |
| Stopping | the windows close. Nothing of Explorer's was hidden, moved or changed, so there is nothing to put back; `rf_win::desktop::cover` has a test that its code calls nothing that could change or message a window that is not ours, and every call that changes a window starts by checking that the window belongs to this process. |

## Known limits

* **One virtual desktop.** The cover windows belong to the virtual desktop they were made on; Windows hides a top-level window of another one while Explorer's own desktop window shows on all of them. On a second virtual desktop the user sees Explorer's desktop and icons, not ours (a rename or delete done on one is true on both, but the picture differs). Showing a window on every desktop needs an undocumented interface; not attempted.
* **Cost at rest is not measured.** Two swap chains of monitor size and the wallpaper pictures stay alive while the switch is on. The dock's own animations redraw only the dock's windows (`cover::refresh_dock_windows`), not the desktops. Working set and idle CPU with the switch on and off are for the live test to measure.
* **Show Desktop is not bound to a key** and its `Driver` keeps its own memory of what it minimized; when the covers come down because a program came to the front, the Driver still remembers (the next toggle restores what is still minimized and lowers an already lowered desktop, which does nothing). The binding increment decides whether that memory should end with it.

## Live test of a test copy

See the report of the increment; in short: `RUSTYFILES_CONFIG_DIR=<temp>`, `RUSTYFILES_MULTI_INSTANCE=1`, `RUSTYFILES_MUTE=1`,
`RUSTYFILES_DESKTOP_DIR=<scratch folder>`, `rustyfiles.exe --shell`, with `shell_desktop_cover = true` in the copy's settings.toml.
