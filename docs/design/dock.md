# The dock (experimental)

`rustyfiles.exe --dock` puts three floating "islands" above the bottom of the primary monitor, in the spirit of a taskbar of our
own. Version 1 is an addition to Windows' taskbar, not a replacement: Explorer keeps the desktop, the Start menu and the
notifications, nothing is injected into anything, nothing needs administrator rights, and everything the dock did is undone when
it exits.

```
 [ Windows logo | Ara ......... | 12° ]      [ app  app  app | bin  downloads ]      [ net vol | 14:05 12.11.2026 | bell ]
        left island                                   middle island (the dock)                  right island
```

## What each island does

| Island | Contents | Clicks |
| --- | --- | --- |
| Left (48 px high) | Windows logo, a search field, the weather | logo: Start menu (the Windows key); search: rustyfiles' quick search; the weather is shown, not clicked |
| Middle (72 px high) | the apps pinned to the Windows taskbar in its order, the other running apps, a thin separator, the Recycle Bin, Downloads | not running: start it; running: bring its last used window forward; in front: minimise it; middle click: another instance |
| Right (48 px high) | network and volume glyphs, the time and date in two lines, a bell | network, volume: quick settings (Win+A); time, bell: notification centre and calendar (Win+N) |

The icon under the pointer grows from 44 to 56 px, its two neighbours to 50 px, eased over about 120 ms (instantly when the
animations setting is off). A dot under an app means it is running; the app in front has a wider dot.

The three windows are always on top, never activated by a click (they do not take the keyboard from the app you are typing in), have
no taskbar button and are not in Alt+Tab. All three have their bottom edge 10 px above the bottom of the monitor's work area, so
with the Windows taskbar visible they float just above it. They hide while a full-screen application (a game, a video) is in front
on the primary monitor and come back when it leaves.

HyperGlass is behind each island, with the island's own corner radius. While the glass is not running (starting, failed) an island
paints its own rounded translucent fill, so the dock is never invisible. It starts from the system's light / dark theme.

**Light or dark foreground.** Over a white page the glass is light grey and white text can hardly be read, so each island looks at
how bright the picture behind its glass is and turns its text, symbols and glass tint the other way. The glass engine measures it
for free: at each draw it reads back one coarse level of the backdrop for its loop guard, and the mean brightness (Rec. 709 weights
on the encoded colours, 0 to 1, only the texels under the glass itself, not its padding) comes from the same bytes
(`rf_win::glass::backdrop_luma(hwnd)`; `None` while unknown). The dock looks at it four times a second. An island switches to
dark text on light glass above 0.58 and back to light text on dark glass below 0.46 (between the two it stays as it is, so a page
near the line does not make it flicker); the colours ease over 300 ms and the engine is given the matching tint. Nothing is drawn
when nothing changed. While the glass is not running, or has not measured yet, the system's theme decides, as before.

## Settings (Settings, "Dock (deneysel)" / "Dock (experimental)")

* **Dock'u göster (deneysel)** / Show the dock (experimental). Off by default. Switching it on starts the process now and at every
  start of rustyfiles; switching it off stops it. The dock keeps running when rustyfiles' window closes: it is your taskbar.
* **Pencereler dock'un altına girmesin** / Keep windows from going under the dock. On by default. The dock registers as an
  application desktop toolbar at the bottom of the primary monitor (the middle island's height plus the margins), so maximised
  windows stop above it, and unregisters on exit. Off: it only floats.
* **Hava durumu şehri** / Weather city. Empty (the default): no weather is shown and **no network request is made at all**. With a
  city, the dock asks Open-Meteo (`geocoding-api.open-meteo.com` once for the place, `api.open-meteo.com` for the temperature) every
  30 minutes. Nothing else leaves the machine: only the city name you typed and the coordinates it returned; no location API is
  used. A failed request shows nothing and is retried after 5 minutes, then 10, 20 and 30; a city Open-Meteo does not know is asked
  about again only once an hour.

## The process

* `--dock` is a process of its own, like `--bar-preview`. It never hands over to a running rustyfiles and never starts DirectLook.
  It owns no tray icon, no hotkey and no name index, does not re-register the file manager or the Open/Save dialogs, and never
  writes the settings: it only reads them.
* It is a single instance of its own group (`rustyfiles-dock`: a named mutex and a named pipe). The pipe carries one-word lines:
  `ping`, `reload` (the settings file changed) and `stop` (undo everything and exit). A second `--dock` greets the first and ends.
* It reads the settings file again when the main rustyfiles tells it a dock setting changed, and every 5 seconds when the file's
  modification time differs (hand edits). A file it cannot read right now is left alone, and the dock keeps the settings it has.
* Resource use: the middle island repaints only when what it shows changed (about 26 of 28 task-list events in a minute change
  nothing: a repaint asks for a frame, and a frame keeps GPUI's vsync thread following the display for a second). With the renderer
  setting on Auto the dock is kept on the graphics card (`settings::renderer_mode`), so a busy card cannot make it build a software
  device; a user who chose the software renderer keeps it. The glass shaders are compiled when rustyfiles is built (`rf-win/build.rs`),
  not at the dock's start.
* `RUSTYFILES_NO_DOCK=1` keeps a rustyfiles from starting or stopping a dock (automated runs, a second copy for testing).
* Updates and uninstalling: the dock holds `rustyfiles.exe` open, so the updater's restart and the uninstall hook ask it to stop
  first; the next start of rustyfiles brings it back while the setting is on.
* Ending: `stop`, or Windows ending the session. The glass is taken off every island, the windows close, the toolbar band is given
  back. If the dock panics, the band is given back from the panic hook before the process aborts. If the dock is killed (Task
  Manager) nothing can run: the band stays registered until Windows notices the dead window (this has not been verified), and the
  dock's next start registers again. If Explorer restarts, the dock hears the `TaskbarCreated` broadcast and registers again.
* Keystrokes (`SendInput`: Win, Win+A, Win+N, and the Alt tap that lets a window come forward) are sent only as the direct answer to
  your click on that element, never at start-up and never from a timer.

## Where things are

* `crates/rf-win/src/dock/`: the system side. `layout.rs` (the three rectangles from a work area and a scale), `model.rs` (merge and
  order of pinned and running apps, the click decision), `identity.rs` (a window's packaged-app id), `pins.rs` (the taskbar's pins,
  their order, and which are alive, moved or dead), `icons.rs` (the shell's icons and the window's own), `hover.rs` (the grow animation), `clock.rs`, `fullscreen.rs` (the full-screen decision), `island.rs` (window styling and
  placing), `system.rs` (display / DPI / theme / work-area watch and the toolbar), `launch.rs`, `triggers.rs`. `weather.rs` has
  the Open-Meteo requests (WinHTTP) and the weather-code mapping.
* `crates/rf-app/src/dock/`: the views and the process. `host.rs` (the three windows, events, settings, the clean exit), `left.rs`,
  `middle.rs`, `right.rs`, `backdrop.rs` (an island's glass), `control.rs` (starting and stopping the process), `plan.rs` (widths,
  layout and timing arithmetic), `look.rs` (colours, and the palette that eases between the dark and the light look), `tone.rs`
  (light or dark foreground from the brightness behind the glass, with its hysteresis), `icons.rs`, `actions.rs`.
* The glass engine takes a per-window corner radius: `rf_win::glass::attach_with(hwnd, GlassOptions { dark, radius_px, cut_corners })`.

## Which apps appear

* A pin and a running window are the same app when their AppUserModelIDs agree (case aside) or the program is the same. The windows
  of a packaged (Store) app often set no id of their own (Telegram, Spotify, WhatsApp, Claude, Windows Terminal), so the id of the
  process's package is used (`identity.rs`); without it the pin and the running app were two items, and the running one had the icon
  of an exe under `WindowsApps` that the shell cannot open (a generic white window).
* A pin whose program is gone is looked for under the next version: the deepest folder of its path with digits in its name is
  looked for beside itself (`Adobe Photoshop 2026` -> `Adobe Photoshop 2027`, newest first, the rest of the path must exist), then
  `App Paths` of Windows, then a Start menu shortcut to a program of the same file name. The pin then stands for that program (and
  starts it). How Explorer itself resolves such a pin was not found out: its link tracking, `IShellLink::Resolve`, the
  shortcut's property store and the `Taskband` data do not point at the new install on this machine; these rules reach the same
  result for Photoshop, Illustrator and After Effects. A pin for which nothing is found is left out. A pin whose program is there
  stays even when the icon file its shortcut names is gone (DaVinci Resolve's installer cache): the taskbar shows it, and the dock
  draws the program's own icon. The Start menu rule only takes a shortcut that stands in a folder of the same name as the old
  program's (the `Update.exe` of one app is not another's).
* An icon that cannot be loaded is tried from the icon one of the app's task windows has set itself (`WM_GETICON` with a timeout, the
  class icon), then from its package's logo; only then is the app's first letter drawn.

## Not in version 1

Hiding or replacing Windows' taskbar, becoming the shell, other monitors' docks, drag to reorder or pin, jump lists, notification
badges, thumbnails on hover. Hiding Explorer's taskbar is a later version and needs a watchdog that brings it back if the dock dies.

## Known limits

* Never run on a screen yet when this was written: the placement, styling, glass, hover and clicks are covered by pure tests of the
  arithmetic and decisions, not by a live run.
* The pinned apps' order comes from the `Taskband` registry value read as text runs, not as a full shell item list; a Windows build
  that changes the record layout gives the shortcuts in name order instead.
* A frameless maximised window is told from a full-screen game by its sizing frame and maximised state; a game that is maximised and
  has a sizing frame would not hide the dock.
