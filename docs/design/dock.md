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
| Middle (72 px high at rest) | the apps pinned to the Windows taskbar in its order, the other running apps, a thin separator, the Recycle Bin, Downloads | not running: start it; running: bring its last used window forward; in front: minimise it; middle click: another instance |
| Right (48 px high) | network and volume glyphs, the time and date in two lines, a bell | network, volume: quick settings (Win+A); time, bell: notification centre and calendar (Win+N) |

A dot under an app means it is running; the app in front has a wider dot.

## Magnification (the middle island)

Like the Mac's dock. Icons rest at 44 px. With the pointer over the island every icon grows by a smooth bell of the horizontal
distance between the pointer and the icon's REST centre (a raised cosine over 2.5 icon slots, 135 px, to each side): the icon under
the pointer to 1.55 times, its neighbours one slot away to about 1.36, two slots away to about 1.05. The size is a continuous
function of the pointer, so moving between icons changes every size a little and never one icon by a step. The icons stand on one
baseline (12 px above the glass's bottom edge) and grow UP: out of the glass, over its top edge, into the window's headroom, where
nothing clips them. They push their neighbours apart so nothing overlaps, and the glass widens with them, symmetrically around the
island's centre; its height never changes. The running dots stay under their icons, the separator and the Recycle Bin / Downloads take
part like any icon. Coming in and going out are eased (smoothstep) over 160 ms, and while the pointer moves inside the sizes follow it
directly; leaving, the bell shrinks away where the pointer last was. With the animations setting off the sizes jump.

Where the glass cannot widen (a narrow screen, the side islands close by: their least gap of 8 px is kept) the gaps between the icons
give way down to 2 px and then the icons shrink together; the icon under the pointer is still the biggest. The icon bitmaps are loaded
once at 44 x 1.55 px times the scale and only ever scaled down, so a big icon is sharp.

Settings, "Dock (deneysel)": "Simge büyütme" / Icon magnification: off, subtle (1.25) or full (1.55, the default).

## Windows, margins and the pointer

The three windows are always on top, never activated by a click (they do not take the keyboard from the app you are typing in), have
no taskbar button and are not in Alt+Tab. The VISIBLE glass of all three has its bottom edge 10 px above the bottom of the monitor's
work area, so with the Windows taskbar visible they float just above it, and the band the desktop toolbar reserves is unchanged.
They hide while a full-screen application (a game, a video) is in front on the primary monitor and come back when it leaves.

Each window is BIGGER than its glass (`layout::placements`): 4 px of transparent margin on every side (where the screen ends the
margin stops there) for the glass's anti aliased edge and the pixel the window region keeps beyond it; the middle window has 12 px of
headroom above its glass for the icons growing out of it (a grown icon reaches 8 px over the glass's top edge) and keeps 40 px more on
each side for the glass to widen. The glass is a SHAPE inside the window (`GlassOptions::shape`), the rest is transparent.

THE POINTER GOES THROUGH WHAT IS NOT THE ISLAND'S. A window region (`SetWindowRgn`, `hit.rs`) is set to the glass (a rounded rectangle,
the shape's own, grown by 1 px so that the glass's anti aliased edge is not cut) plus the boxes of the icons that stand out of the
glass, and is updated at every frame of the magnification (the region first, then the glass, in one task, so the system never clips a
glass that has just grown). The system treats everything outside the region as not being part of the window, for painting and for hit
testing, so a click on the margin goes to whatever lies below, also in another process (the Windows taskbar under the dock is
Explorer's). `WM_NCHITTEST` / `HTTRANSPARENT` was not used: it passes only to windows of the same thread.
`WS_EX_LAYERED | WS_EX_TRANSPARENT` was not used: it would take the island's own clicks too, and the per pixel alpha of a layered
window is not what DirectComposition draws. While a mouse button is held on the dock the icons stand still, so a click that begins
during the 160 ms of the bell coming in ends on the icon it began on.

THE SHADOW AND THE POINTER EXCLUDE EACH OTHER, and the dock chooses the pointer. The region is also the system's clip (Microsoft: the
area where the system permits drawing), and a shadow lies outside the glass, so it is expected to be clipped away; a region wide
enough to keep it would be a band 22 px to the sides and above and 26 px below that takes the clicks meant for the taskbar and for the
windows above the dock. So by default the islands draw NO shadow and their windows keep only the edge room described above (under half the pixels of a
window with room for the shadow, which also costs region textures and swap chains). The engine can draw it
(`GlassOptions::shadow`, tested offscreen: black 26 % at the outline, 4 px down, gone 22 px out, never under the glass), and
`RUSTYFILES_DOCK_SHADOW=1` starts the dock with 28 px of room around every glass and the shadow on, to see on a real screen whether the
system really clips it. If it does not, the switch can become the default; if it does, a shadow needs a window of its own that takes no
input (not built).

## The glass

HyperGlass is behind each island: the same engine and the same look as the main window (the four Settings sliders reach the dock:
at its start from the settings file, and when the file changes, which rustyfiles announces to the dock for every slider step once the
slider has settled). A small piece has the same optics, frost, tint, saturation and rim light as a window; the only difference is
that its rim band is bounded by its size, at most a third of its short side (16 px for the 48 px islands, 24 px for the 72 px one;
a window's is 44 px): without the bound the band of a 48 px island was nearly all of it and the glass looked like marble. The rule is
the engine's, by the glass's short side below 160 CSS px, so it also holds for the popups drawn on HyperGlass (menus, dropdown lists,
flyouts: `glass_watch.rs`): a 196 x 122 popup keeps a calm band of about 41 px across its middle, where text is read. What is
seen: a bending band along the outline, a thin bright rim light, a frosted centre for text and icons. While the glass is not running
(starting, failed) an island paints its own rounded translucent fill, inside the glass's rectangle of its (larger) window and in the
colours of the island's current tone, so the dock is never invisible. It starts from the system's light / dark theme.

**Light or dark foreground.** Over a white page the glass is light grey and white text can hardly be read, so each island looks at
how bright the picture behind its glass is and turns its text, symbols and glass tint the other way. The glass engine measures it
for free: at each draw that looks at the backdrop it reads back one coarse level of it for its loop guard, and the mean brightness
(Rec. 709 weights on the encoded colours, 0 to 1) comes from the same bytes (`rf_win::glass::backdrop_luma(hwnd)`; `None` while
unknown). Which texels count is ONE definition for the brightness and for the loop guard's fingerprint: the texels under the glass
(`Geometry::glass_rect`: the whole window for a window, the SHAPE for an island in its larger window), the fingerprint with the
glass's padding added, the brightness without it. So a bright window beside an island, or in the transparent margin of its own window,
neither turns the island's text dark nor keeps the glass drawing. (The brightness is the mean over the shape's bounding box; the
four rounded corners are not cut out.) A draw that only moves the shape, which uses the backdrop it already has and reads nothing
back, leaves the last brightness as it was; the next draw that looks at the backdrop measures the new rectangle. The dock looks at
the value four times a second. An island switches to dark text on light glass above 0.58 and back to light text on dark glass
below 0.46 (between the two it stays as it is, so a page near the line does not make it flicker); the colours ease over 300 ms and
the engine is given the matching tint (`set_dark`). Nothing is drawn when nothing changed. While the glass is not running, or has
not measured yet, the system's theme decides.

The tint and the shape are two changes of ONE options value of an island (`IslandGlass`): a change of the shape is sent with the
tint the engine has, a change of the scale with both, and the engine's `set_dark` changes the tint only, so neither undoes the
other. A change of the tint is a new backdrop pass (the glass tint differs); a change of the shape alone is the cheap draw below.

What the animation costs the engine: one constant buffer upload and one draw of the window's size per frame, and one `Present`. The
backdrop did not change, so no copy of the screen, no blur pyramid, no read back and no new texture or swap chain (a change of the
shape alone is a different case from a change of the backdrop; `Win::set_options`). The engine is NOT paced by the display for it
(`DwmFlush` would hand every shape to the engine one refresh after the app drew its scene): it takes each shape the moment it
arrives, a capture session's wait ends on a command and a Desktop Duplication wait is cut to 2 ms for 120 ms after a change, and
presents at once, so the glass lands in the same composition as the icons. At rest nothing runs. The app side per frame: one
layout (a few small vectors), one shape for the engine, one region for the system, no icon loading; a mouse move that changes nothing
visible (the magnification off, or a quarter of a pixel while the bell is settled) draws nothing. In Desktop Duplication mode (the
fallback) the compositor may report a new frame after our own `Present`, which restales the islands once (a copy and a pyramid at
the window's size, then the loop guard skips the draw); this is the same for a moving lens and has not been measured.

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

* `crates/rf-win/src/dock/`: the system side. `layout.rs` (the three glass rectangles and their windows from a work area and a scale), `model.rs` (merge and
  order of pinned and running apps, the click decision), `identity.rs` (a window's packaged-app id), `pins.rs` (the taskbar's pins,
  their order, and which are alive, moved or dead), `icons.rs` (the shell's icons and the window's own, as a chain of sources per app),
  `magnify.rs` (the magnification), `hit.rs` (which part of a window takes the pointer, and the window region), `hover.rs` (the
  slots), `clock.rs`, `fullscreen.rs` (the full-screen decision), `island.rs` (window styling and
  placing), `system.rs` (display / DPI / theme / work-area watch and the toolbar), `launch.rs`, `triggers.rs`. `weather.rs` has
  the Open-Meteo requests (WinHTTP) and the weather-code mapping. Every call into the shell runs on the one thread that pumps
  messages (`rf_win::sta::call`).
* `crates/rf-app/src/dock/`: the views and the process. `host.rs` (the three windows, events, settings, the clean exit), `left.rs`,
  `middle.rs`, `right.rs`, `backdrop.rs` (an island's glass; its state machine is `glass_watch.rs`, shared with the popups), `control.rs`
  (starting and stopping the process), `plan.rs` (widths, layout and timing arithmetic), `look.rs` (colours, and the palette that eases
  between the dark and the light look), `tone.rs` (light or dark foreground from the brightness behind the glass, with its hysteresis),
  `icons.rs` (the icon cache, keyed by an app's first source), `actions.rs`.
* The glass engine takes a per-window corner radius, a shape and (off for the dock by default) a shadow:
  `rf_win::glass::attach_with(hwnd, GlassOptions { dark, radius_px, cut_corners, shape, shadow })`; the shape follows the glass through
  `rf_win::glass::set_options` (cheap, made for animation) and the tint through `rf_win::glass::set_dark`.

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

* What has been seen on a screen: the placement, the glass, the apps and their icons, the light or dark foreground (main's live runs,
  before the magnification was added). What has not: the magnification, the glass as a shape inside a larger window, the window
  region and the clicks through it, and the merged whole; they are covered by pure tests of the arithmetic and decisions and by
  offscreen pictures of the glass. To look at: whether the
  shadow shows with `RUSTYFILES_DOCK_SHADOW=1` (expected not: see above), the glass following the icons frame by frame, the top of a
  grown icon while it is still growing (the region follows the scene by one task of the message loop and is a pixel larger than it),
  the corners of the region (a region has no anti aliasing; the glass's own edge is inside it), and clicks on the margin reaching
  the taskbar.
* The pinned apps' order comes from the `Taskband` registry value read as text runs, not as a full shell item list; a Windows build
  that changes the record layout gives the shortcuts in name order instead.
* A frameless maximised window is told from a full-screen game by its sizing frame and maximised state; a game that is maximised and
  has a sizing frame would not hide the dock.
