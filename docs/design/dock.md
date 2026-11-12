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
| Left (48 px high) | Windows logo, a search field, the weather | logo: our own Start panel (see "Panels, Start and search"; with "rustyfiles Başlat menüsünü kullan" off, the Windows key as before); search field: the same panel with the keyboard in its field, and the text typed there is shown in the field on the island (off: rustyfiles' quick search window); the weather is shown, not clicked |
| Middle (72 px high at rest) | the apps pinned to the Windows taskbar in its order, the other running apps, a thin separator, the Recycle Bin, Downloads | not running: start it; running: bring its last used window forward; in front: minimise it; middle click: another instance; right click: the dock's own app menu; the pointer resting on a running app: a preview of its windows (see "The app menu, the hover previews and the dock's own pins") |
| Right (48 px high) | (with a tray source only: the other programs' notification icons and a chevron, see "The tray strip and its overflow panel") network and volume glyphs that show the real state (the volume glyph shows mute), the time and date in two lines, a bell | network, volume: our own control centre (see "The control centre"; with "rustyfiles denetim merkezini kullan" off, Windows' quick settings, Win+A, as in version 1); time: our own calendar panel (see "The calendar and the notification panel"; with "rustyfiles takvimini kullan" off, Windows' notification centre and calendar, Win+N, as before); bell: our own notification panel, with a small dot while there are notifications (with "rustyfiles bildirim panelini kullan" off, Win+N as before) |

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

## Panels, Start and search

A panel is a borderless pop-up window of the dock process on HyperGlass that opens 8 px above an island's visible glass, lined up with
its left edge (left island) or right edge (right island), inside the monitor's work area (`rf_win::dock::panel::place`). The island's
visible glass is `host::visible_glass_rect`: the glass rectangle of the layout (`Placement::glass`, screen coordinates, device px),
NOT the island's window, which is larger (a transparent margin, the middle island's headroom) and keeps the system's invisible
frame. At 100 % the left island's glass is (12, 974, 342, 1022) on a 1920 x 1080 monitor and its window (8, 970, 346, 1026): the Start
panel's bottom edge is 966 (8 px above the glass), not 962; on a 2560 x 1440 monitor with the 48 px taskbar the left glass is
(12, 1334, 342, 1382) and the Start panel (12, 646, 632, 1326), the control centre (2188, 926, 2548, 1326). The rectangle is where
the panel's CLIENT area goes (what it draws and what its glass covers): `rf_win::dock::panel::show_at` measures how far the window's
outer rectangle reaches beyond its client area (the islands' windows keep the system's invisible resize frame, 8 px left, right and
below at 100 %; the panels are the same kind of pop-up and may too, nothing is changed if they do not) and sets the outer rectangle that gives exactly that client area. This was not seen
on a screen. The same rectangle is what the open panel's outside-click test keeps (a
click on the island's own logo, field or symbol is the island's button, which toggles the panel; a click on the island's transparent
margin is another program's and closes the panel). Only one panel is open at a time. Unlike the islands a panel IS activated: it takes the keyboard while it is open, and when it is put away with Escape
or by its button the window that had the keyboard gets it back (after a click elsewhere, a thing started from the panel or a window
taking the foreground, the foreground is left alone). It closes on Escape, on a mouse button going down outside it (not on its own
island: that button toggles it), when another window takes the foreground, when the dock hides for a full-screen application, when
the monitor, its scale or its work area changes (the islands move; a panel left open would stay behind), on Alt+F4 (the window is
only put away: a destroyed panel window is never opened again) and when its button is clicked again. Opening and closing ease over
120 ms (opacity and a 6 px rise; at once with animations off). The panel's glass is the popups' (`glass_watch::PopupGlass`: a plain
fill until the engine has drawn, for good if it fails) but with the system's ordinary round corners (`DWMWCP_ROUND`, 8 px at 100 %,
`PopupGlass::start_panel`; menus and dropdowns keep the small 4 px ones), dark or light with the tone its island has at the moment the panel opens (`IslandGlass::dark`, the island's target tone: the brightness behind its glass with the system's theme as the fallback); the panel's own views paint with the island's palette (`host::frame_palette`). The panel is brought
to the front without the last-resort tap of the Alt key that `bring_to_front` has (that tap goes into whatever window is in front and
can light up its menu bar); if Windows refuses the foreground the panel closes itself by the 1.2 s foreground rule.

**While a choice is being carried out the panel does nothing more.** A start, a power action or the like sets a `closing` flag at once:
clicks and keys are ignored while the panel eases out, so a double click on an app starts it once, and a held Enter does not repeat.
A click that removes a menu or answers the question starts a 350 ms quiet period, so the second click of a double click cannot land on
the row that lies under the place where the menu was. The start itself runs off the UI thread; the panel waits up to 300 ms for it: a
failure that arrives within that keeps the panel open with a line above the bottom row ("Başlatılamadı. Kaldırılmış ya da taşınmış
olabilir.", "Bu işlem yapılamadı. Yeniden deneyin."), a call that takes longer (a sleep returns only when the PC wakes, Windows' consent
prompt waits for the user) is not waited for, and declining the consent prompt is not a failure.

**What it costs.** Each panel's window (the Start panel's, the control centre's, the calendar's, the notifications') is made WHEN THE
PANEL IS FIRST OPENED and kept hidden after that (it used to be made at the dock's start): a dock whose user never opens a panel holds
no window, no swap chain and no view tree for it. The click that asks for the panel makes the window from a task of the foreground
executor (`panel_more::toggle_made`, with its `Making` guard so that a second click before the window exists does not toggle the panel
shut again), then opens it; every later opening is a move and a show. The time the user waited is logged at info level, from the
click to the panel being on the screen with its glass attached:
`dock: panel start window made in 31ms from the click` (only the first time) and `dock: panel start shown in 74ms from the click
(window made for it)`. There is no timer, no thread and no glass of a closed panel (the control centre's sources run only while it is
open; the sound and the network are followed all the time, as the right island shows them). The glass engine is attached when the panel
opens and taken off after it has closed. While it is open one 50 ms task looks at the foreground and the mouse buttons. The Start
panel's app list, pins and the user's name are read when the panel is first made (about 0.4 s for 273 apps on the machine this was
written on: the first opening shows the empty list for that long, where the old four-second warm-up after the dock's start had it
ready; this is the price of making nothing at the start).

**The Start panel** (620 x 680 logical px; the logo, and the search field, open it). Top: the search field. Empty query:
"Sabitlenenler" (the user's pinned apps, a grid of six) and "Tüm uygulamalar" (every installed app in the user's alphabet, Turkish
letters in the Turkish order, with letter headers, in the app's virtual list; an icon is asked for only for rows in view, through the
dock's icon cache and the shell thread). Right click an app anywhere: pin / unpin ("Başlat'a sabitle", "Sabitlemeyi kaldır"), run as
administrator (Windows' consent prompt, because that is what the user asked for), open the program's place in rustyfiles. Bottom: the
user's name, Settings (rustyfiles' own), the power button (Kilitle, Uyku, Oturumu kapat, Yeniden başlat, Bilgisayarı kapat; the last
two ask "Emin misiniz?" in the panel first). The question is a small box in the MIDDLE of the panel, not where the menu's rows were
(it appears under the pointer that chose "Yeniden başlat" or "Bilgisayarı kapat", and the second click of a double click must not
answer it), "Evet" is drawn faint and does not count for the first 600 ms (`power::yes_counts`), and the keyboard starts on "Hayır"
(Left / Right move, Enter answers what it is on). The pins are `start-pins.json` in the config folder: seeded once from the taskbar's
pins (matched by AppUserModelID, program or name), then the user's own; an app that is not installed any more stays in the file and out
of the grid. A seeding whose taskbar read did not come back in time writes nothing and is tried again at the next opening (an empty
list from a failed read must not become the user's choice for good); a pin file that exists and cannot be read is moved aside
(`start-pins.json.broken`), never written over; writes are numbered and a stale snapshot never overwrites a newer one. The pinned grid
shows three rows and scrolls beyond them; the keyboard cursor scrolls it into view. Keyboard: typing goes to the field (also from the
bottom row's buttons), the arrows move through the grid and the list, Enter starts, Ctrl+Enter shows the program in rustyfiles (the
footer offers it only for a file or an app), Escape clears the query first and closes second, Tab goes field, Settings, power, the Menu
key or Shift+F10 opens the app menu of the thing under the cursor (arrows and Enter in it), and the power menu takes the arrows and
Enter. The keyboard cursor is the app's selection pill, like every list. Menus are drawn inside the panel (a window of their own
would take the activation and close it), placed from the panel's real size: the window is shorter than 680 px on a small screen.
There is no key that opens the panel: the logo and the search field are the way in, and the Windows key still opens Windows' own
Start menu (a keyboard hook in the shell's process is a decision of its own, not made here).

**Search.** The sources and the ranking are the quick search's own (`quick_search::search_local`: apps, bookmarks, the web;
`quick_search::search_files`: the name index), shared, not copied. The dock process does not hold the name index (memory), so the
files come from the running rustyfiles over pipes (`search_pipe.rs`): the dock writes `\x01qs-files <id> <query>` to the main app's
SEARCH pipe (`rustyfiles-search`, served only by a main app that knows the protocol: on the ordinary instance pipe a main app from
before this would read every keystroke as a launch argument, open an error tab and raise its window; with an older main app running
the dock simply finds no search pipe and says "no main app"), the main app searches its index on a thread of its own and writes
`\x01qs-files-reply <id> <state> <hits>` to the dock's pipe (these pipes are one-way, so there are two). Apps and the rest answer within a frame or two from a worker thread
(about 1.2 ms per keystroke in a debug build, 273 apps and the bookmarks); the files arrive after them and slot in after the apps
without moving the row the user has moved to (the selection follows the row, not its number), and an Enter before they arrive waits
for them only when the first row is not an app. The files of the previous query stay on the screen until the answer for the new query
replaces them (rows that vanish at every keystroke and come back make the list jump). If no rustyfiles is running there is one row
that says so and starts it without a window (`rustyfiles.exe --no-window`) when chosen; the query is asked again when it answers. That
process has no window and no tray icon, so it ends itself after 10 minutes without a request for files, if no window has been opened
in it by then (`search_pipe::watch_idle`; a window opened in it makes it an ordinary rustyfiles). The field in the left island shows what is
typed in the panel (the island is never activated, so it cannot take keys itself).

## The control centre

The network and volume glyphs of the right island open our own panel instead of Windows' quick settings. It is the same machinery as
the Start panel (`panel.rs`: one slot per panel, placed 8 px above the island's visible glass, lined up with its right edge, one panel
open at a time), about 360 logical px wide and as tall as what it shows, on HyperGlass (`glass_watch::PopupGlass`, a plain fill until
the engine has drawn) with the liquid controls (`liquid::slider_parts`). The clock and the bell are NOT part of it: they still open
Windows' notification centre and calendar (Win+N), which is a later piece of work.

```
 [ tile: Network ] [ tile: Bluetooth ]        connectivity tiles, two to a row ("Pil" joins them on a laptop)
 [ art  Spotify  |<  >  >| ]                 now playing (only while a media session exists)
 [ outputs: Hoparlör, Kulaklık ... ]         the list of outputs (a card of its own, only while open)
 [ (speaker) ----o------ (^) ]                sound: mute, volume slider, the outputs
   Analogue 1/2 (Audient iD4)
 [ (sun)     ----o------     ]                brightness (only when the monitor answers DDC/CI)
   Parlaklık
                         (gear)               Windows' Settings
```

* **Tiles.** Network: the Wi-Fi name and its strength in a word (Zayıf / Orta / Güçlü), "Ethernet", "Bağlı değil" or "İnternet yok";
  Bluetooth: off, on, or the connected device's name (the first and "+n"); Battery (only when the machine has one): the percent and
  charging / plugged in / battery saver. A tile whose source is not on the machine (no Bluetooth radio, no battery, no network source)
  is left out, never shown dead. A tile opens Windows' page for the thing (`ms-settings:network`, `ms-settings:bluetooth`,
  `ms-settings:powersleep`) and puts the panel away: **nothing is switched from here.** Wi-Fi and Bluetooth on / off is
  `Radio.SetStateAsync`, which the research note marks as not verified for a program with no package identity, and trying it would
  change the user's radios, so it is not in this version.
* **Now playing.** The system media session (`GlobalSystemMediaTransportControlsSessionManager`), the one Windows' own flyout shows.
  It works for an unpackaged program (checked on this machine: the manager activates, a session is listed, its properties and
  thumbnail read). Artwork, title, artist, the program's short name and previous / play-pause / next for the session Windows calls
  current; choosing between several sessions is not offered. The artwork is decoded on the media thread and cut to a 128 px square.
  A track whose thumbnail has not come yet keeps the previous picture of the same program for ONE more look (Windows' event, or 1.5 s
  later), and is read again until a picture has been decoded for this very track; if none comes the cover is dropped rather than left
  on the wrong song (`media::ArtState`, tested).
* **Sound.** A liquid slider for the master volume with the speaker at its left (a click mutes; raising the volume of a muted
  speaker unmutes, as Windows' slider does; two quick clicks flip the state the first one asked for, not the stale snapshot) and the
  default output's name under it. Changes made elsewhere (the keyboard's volume keys,
  another program, Windows' own panel) move the slider at once (`IAudioEndpointVolumeCallback`, `IMMNotificationClient` for devices
  coming, going and the default changing). A drag sets the volume continuously without waiting: the COM call is made on the audio
  thread, one at a time, and only the newest value of the drag is applied. The slider takes the endpoint's own positions
  (`GetVolumeStepInfo`; the interface on the development machine reports 51, so 2 % steps). The chevron opens a list of the active
  outputs with the default one ticked, **read-only**: switching the default device is `IPolicyConfig`, undocumented and fragile (the
  research note marks it so). The list is plain lines (they do nothing, so they do not look like choices) and ends with one row that
  does something: "Çıkışı değiştir: Ses ayarları" opens Windows' sound page. It is a card ABOVE the sound card, and the window grows
  upwards, so the chevron the user has just clicked (and the slider) stay under the pointer; its arrow points up while it is closed.
* **Brightness.** DDC/CI of the primary monitor (`dxva2`). Read once when the panel opens (a transfer takes tens of milliseconds,
  and there is no event for a monitor's own buttons), written only by the slider. The row appears only when the monitor answers; a
  laptop's own panel does not answer DDC/CI (it needs WMI, which is **not implemented**), so there the row is left out.
* **Gear.** Opens Windows' Settings home.
* **The keyboard.** Every control is a tab stop (the tiles, the speaker, the volume slider, the outputs button, the media buttons that
  can be pressed, the brightness slider, the gear) with a ring in the accent colour while it has the focus by key; Enter and Space press
  a button; a slider takes the arrows (one position), Shift or the page keys (ten), Home and End; Escape closes. The icon-only buttons
  have tooltips. The colours are the app's own palette for the island's tone (its accent, lifted on dark, and the text colour that
  goes on it).

**What runs, and when.** `rf_win::sysstatus` has one module per source (snapshot struct = plain data; a read; a follow function that
calls back from the source's own thread on a system event; no polling loops):

| source | events | runs |
|---|---|---|
| `audio` | `IAudioEndpointVolumeCallback`, `IMMNotificationClient` | one MTA thread from the dock's start, blocked on its channel when nothing happens: the island shows the volume glyph all the time |
| `network` | `NetworkStatusChanged` | one MTA thread from the dock's start: the island shows the network glyph all the time. Windows has no event for a Wi-Fi signal that only changes in strength, so the panel reads again when it opens and every 5 s while it is open |
| `media` | sessions, current session, playback info, media properties | one MTA thread while the panel is open |
| `bluetooth` | `Radio.StateChanged`, a `DeviceWatcher` (classic and low energy) | one MTA thread while the panel is open |
| `power` | `RegisterPowerSettingNotification`, callback form (no window) | nothing of ours; registered only on a machine with a battery, only while the panel is open |
| `brightness` | none | one MTA thread while the panel is open |

None of these threads is the shell thread, and none is a single-threaded apartment that does not pump: audio and WinRT want MTA.
The callbacks only post a message and return. The app side (`dock/sysstate.rs`) hands the audio and network snapshots to the app's
thread through a channel and keeps them in a global that the island's glyphs and the open panel read; the panel's other sources
(`control_centre.rs`) are started when it opens and dropped when it is hidden, and what they last said is kept so that the next opening
starts with the same cards and corrects them within a moment. The panel's height follows what it shows (`cc_model::height`); a card
that appears while it is open (the first reading of Bluetooth or the brightness, a media session starting) places the window again,
growing upwards (`panel::relayout`).

**The glyphs.** Volume: muted, no device, level 0, up to a half, above a half. Network: not connected, Ethernet, Wi-Fi in four
strengths (Windows' five bars as none / low / medium / full), something else. Both follow the state, not a fixed symbol.

**Pure and tested.** The volume to slider and back with the endpoint's positions, the glyph from a snapshot, the text of each tile in
Turkish and in English, the coalescing of slider writes (`Latest`), which tiles a machine gets, the panel's height for what is shown,
the pointer to slider arithmetic, the brightness range mapping, the program name of a media session, the artwork cut, the battery
status decoding. `cargo run -p rf-win --example sysstatus_probe` prints what each source reports on the machine it runs on (read-only;
no titles).

## The calendar and the notification panel

Two more panels of the same machinery (`panel.rs`; their slots are in `panel_more.rs`): the same placement above the right island's
glass (8 px up, lined up with its right edge, kept inside the work area), the same closing rules (Escape, a click elsewhere, another
window in front, a layout change, the button again), one panel at a time, the glass, the island's tone, the 120 ms easing. Each has
its own setting (below) and falls back to Win+N when it is off. A panel's window is made at its first click (see "What it costs" above), so a
setting switched on while the dock runs needs no special case.

### The clock's panel (a click on the clock)

* Top: today's long date ("12 Kasım 2026 Perşembe" / "Thursday, November 12, 2026"). Then the month's name with the buttons to the
  month before and after (a click on the name goes back to today), the month grid, the time WITH SECONDS in the user's own time
  pattern, and "Tarih ve saat ayarları" (opens `ms-settings:dateandtime`).
* The grid is always six rows of seven (the panel does not change its height from month to month). The week starts on the user's own
  first day (`GetLocaleInfoEx(LOCALE_IFIRSTDAYOFWEEK)`); week numbers are off. Today is filled with the accent colour, the days of the
  neighbouring months are dimmed, the chosen day (the keyboard's) has a ring. A click on a day chooses it; on a dimmed day, moves to
  its month. The years are 1900 to 2100; the month buttons stop at the ends.
* Keys: the arrows move the chosen day (left and right a day, up and down a week: the day above or below in the grid), Page Up and Page
  Down a month, Home goes to today, Escape closes, Tab visits the month buttons, the name and the link.
* NAMES come from the app's own Turkish and English tables, chosen by the app's language and not by the Windows region (the rest of the
  panel speaks the app's language; a mix would read badly). Any other language of the app gets the English table.
* NOT in this version: an agenda (there is no source of events that works without an account), other time zones, alarms.
* The seconds tick only while the panel is open: one task that wakes at each second, ended when the window is hidden.
* Pure and tested (`rf_win::dock::calendar`, `cal_view.rs`): leap years (1900 no, 2000 yes, 2100 no), the weekday of every day of 1900 to
  2100, month grids for all seven first days of the week, adding days and months with the ends held, the names, the keys.

### The bell's panel (a click on the bell)

* It shows the notifications other programs have posted, read through the WinRT `UserNotificationListener` WITHOUT package identity.
  What was verified on this machine (build 26300, from a program with no identity): `GetAccessStatus()` and
  `GetNotificationsAsync(Toast)` work (about 10 ms a call), the push event `NotificationChanged` throws `0x80070490`. So the list is
  POLLED: when the panel opens and every 2 seconds while it is open, never while it is closed (the reader is a thread of its own, COM
  MTA, ended when the panel is hidden).
* ACCESS. Only `GetAccessStatus()` is called; the call that makes Windows show a consent prompt (`RequestAccessAsync`) is never made
  (a test reads the module's source and fails if it ever appears in code), and the list is read only when the status is `Allowed`.
  Otherwise the panel says it in one line ("Bildirimlere erişim kapalı. Windows'ta izin verin.") with a button that opens
  `ms-settings:privacy-notifications`.
* EACH NOTIFICATION: its program's icon (the dock's icon code, on the shell thread, by the AppUserModelID) and name as a heading, then
  its title, up to three lines of text and its age ("şimdi", "5 dk", "2 sa", "dün", else the date in the user's format). Grouped by
  program, the program with the newest notification first, newest first inside a program. A click starts the program (by its
  AppUserModelID, as the Start panel does); it does not press any button of the toast (no API for that). With none: "Bildirim yok".
* REMOVING. A small x on a notification (shown when the pointer is on it or the keyboard is) calls `RemoveNotification(id)`, and
  "Tümünü temizle" calls `ClearNotifications()`. Both are documented, but whether they work for a program without package identity
  was NOT verified (they change the user's list, so the research did not call them). So they are OFF in the real mode (no x, no
  "Tümünü temizle", Delete does nothing) until `RUSTYFILES_TOASTS_REMOVE=1` is set, which is how the first live try is made; making
  it the default is a decision for after that try. With it on, `rf_win::toasts::RemoveWatch` hides the x and "clear all" for the rest
  of the opening when a call returns an error, or when a notification asked to go is still listed after two more polls and no
  removal has been seen to work. In the made-up mode below they work on the made-up list without the variable.
* THE BELL'S DOT is a count of the list, kept while the panel is closed by ONE cheap signal: a plain directory-change wait
  (`ReadDirectoryChangesW`, no polling) on `%LOCALAPPDATA%\Microsoft\Windows\Notifications`, where the notification database lives. A
  change there only triggers one count (after the folder settles for 0.8 s, never later than 3 s after the burst began, and never
  within 3 s of the last read: `rf_win::toasts::read_at`, kept by `HintTimer`). While the panel is open the worker is paused (the
  panel polls the list itself), and the panel's closing makes it read once for what changed meanwhile; the panel also tells the count
  at each of its readings. That every new notification touches that folder is an observation of the research, not a promise of
  Windows. How much it costs on a busy machine is NOT measured: in one 85 s observation with no new notification on the test PC the
  folder was silent (0 change bursts), which says nothing about a machine whose programs update tiles or badges all day; the worst
  case is one count (about 10 ms of a worker thread) per 3 s. If the folder cannot be watched, the dot is only as fresh as the last
  time the panel was open.
* PRIVACY. The texts of notifications are never logged and never written to a file (`Toast`'s `Debug` prints only an id and the
  lengths). They live in memory while the panel is open and are dropped when the window is hidden (the view lets go of them, and the hidden
  window is drawn twice with nothing in it so that GPUI's copies of the last frame, its elements and its shaped text, go too:
  `panel_more::scrub`, not yet seen live); what stays is the count. A notification Windows gave no time shows no age.
  A read of the list that fails is not an empty list: the panel keeps what it shows and the dot keeps its count until a read works.
* NO FLICKER. A poll that finds the same list (`rf_win::toasts::diff` is empty, whatever order Windows lists them in) redraws
  nothing. A poll that finds a different list keeps the keyboard cursor on its notification (or moves it to the one that followed).
* Keys: Up and Down walk the notifications, Enter starts the program of the one under the cursor, Delete removes it, Escape closes.
* Pure and tested (`rf_win::toasts`, `notif_model.rs`): the access decision, grouping and order, the three-line clamp, the age in both
  languages, the difference between two polls, the keyboard cursor across polls, the remove watch, a poll that failed, the timing of the dot's reads and its pause, the one answer for "is the x offered", the age of a notification with no time, the
  panel's height. `cargo run -p rf-win --example toasts_probe` prints COUNTS only (never a title, a text or a program's name).
* TRYING IT WITHOUT TOUCHING THE REAL LIST. `RUSTYFILES_TOASTS_FAKE=many` makes the reader answer with five invented notifications
  (three programs, a long title, a five-line text, ages from seconds to days) that the x, Delete and "Tümünü temizle" really remove,
  in memory; `empty`, `denied` and `notasked` show the other states. In this mode Windows is not asked at all and nothing is changed
  there. Off unless the variable is set.
* WHAT THIS CANNOT DO (unchanged from the research): press the buttons or type in the reply boxes of other programs' toasts; stop
  Windows' own banners (beside Explorer they still appear; the panel is the list, not a replacement for the pop-ups); get the push
  event without a package identity.

## The app menu, the hover previews and the dock's own pins

Three things that belong to the middle island's icons (shell plan increment B8). Each of the two pop-ups has its window made when first
needed, kept hidden afterwards, and is placed above the ICON (`rf_win::dock::preview::above_icon`: centred on the icon, 16 px above the
dock's glass, kept inside the work area), not lined up with an island's edge like the panels.

### The app menu (a right click on an app's icon; setting "Simge sağ tık menüsü", on)

On glass above the icon, top to bottom, like the taskbar's: the app's jump-list SECTIONS (Tasks, the app's own categories, Pinned,
Recent or Frequent, as the source orders and names them, a title above each that has one), a separator, and the FIXED part: the app's
name with its picture (starts a new instance, as a middle click does), "Dock'a sabitle" / "Dock'tan kaldır", and for a running app
"Pencereyi kapat" (one window) or "Tüm pencereleri kapat (n)" (the windows are asked to close the way their own close button does,
`WM_SYSCOMMAND` with `SC_CLOSE`, posted: the program may ask about unsaved work, and one that has no close command is not sent one).
The rows and the keyboard's walk are `rf_win::dock::appmenu` (pure, tested).

* A WINDOW IS ACTED ON ONLY WHILE IT IS STILL THE ONE SEEN (`rf_win::dock::winops::WinRef`: the handle and the process it belonged to
  when the menu opened or the pointer came onto the icon; the close request, a card's activation and the card list check that the
  handle still belongs to that process and is visible). A menu left open on the keyboard for minutes cannot close a window of another
  program that was given the number of a destroyed one.
* A LONG JUMP LIST IS CUT, never scrolled or clipped: at most 10 lines to a section, and fewer when the menu would be taller than the
  room above the dock; lines go from the end of the last section first (`appmenu::fit_entries`), so the fixed rows at the bottom are
  always on the screen and the keyboard only reaches rows that are.

* THE MENU OPENS AT ONCE with the fixed part; the sections are asked for on a worker thread afterwards and appear when they arrive
  (the window is anchored by its bottom edge, so it grows upwards). A highlighted row stays highlighted. Nothing waits for them.
* THE JUMP-LIST SEAM is `jump_source.rs`: a section is a title and entries, an entry is a title, an optional icon and an opaque action
  (`Action(String)`). `sections(&JumpApp)` and `run(&JumpApp, &Action)` are the only two functions the menu calls; here they return
  nothing and do nothing, and wiring them to `rf_win::jumplists` is a matter of those two bodies. Both are called from worker threads.
* KEYBOARD: Down, Up, Tab (Shift+Tab), Home, End, Enter or Space on the highlighted row, Escape. The menu IS activated (it needs the
  keys) and gives the keyboard back to the window that had it when it is put away with Escape.
* CLOSES on Escape, on a mouse button going down anywhere outside it (the dock's own icons too: a right click on another icon closes
  this menu and that click's release opens the other icon's), when another window takes the foreground (after the 1.2 s grace the panels
  have), when the dock hides for a full-screen application, when the monitor, its scale or the glass's top changes, on Alt+F4 (only put
  away) and when a row has been chosen. One menu at a time. While it is open the hover previews are blocked.
* LOG (info): `dock: app menu shown in <time> from the click at x=<device px>` (with "window made for it" the first time).
* Only apps have a menu; a right click on the Recycle Bin or Downloads does nothing, as before.

### The dock's own pin list (`own_pins.rs`, `rf_win::dock::ownpins`, file `dock-pins.json` in the config folder)

Until the user pins or unpins from the dock's menu, the middle island shows the apps pinned to the Windows taskbar, read-only, as it
always has (so nothing changes for a user who does not use the menu). The FIRST pin or unpin copies the taskbar's pins into
`dock-pins.json` (once, up to 32), applies the change, writes the file atomically and makes the island read it again at once; from
then on the file is the list, the taskbar's pins are not read for the island and an empty list is the user's choice. The seeded shortcuts are COPIED into
`dock-pins/` beside the file, so that unpinning in Windows does not take the dock's pin away. WINDOWS' STORES
(the taskbar's pin folder, `Taskband`) ARE NEVER WRITTEN. An entry holds the name, the identities a running window is matched by (the
AppUserModelID, the program), how to start it (shortcut, program with arguments, package id) and where its icon comes from; a pin
whose program is gone stays in the file and out of the island. A file that exists and cannot be read is moved aside
(`dock-pins.json.broken`), never written over, and the island follows the taskbar meanwhile; a seeding whose taskbar read did not come
back in time writes nothing, and neither does one whose read came back EMPTY while the pin folder could not be listed or held shortcuts
that could not be read (`ownpins::empty_read_is_believable`: that would make an empty list the user's list for good). The shortcuts are
copied only after the change is known to change something (a change that changes nothing leaves no copy and no file). Changes are made
one at a time under a lock, each reading the file again.

An app is found again among the pins by its AppUserModelID, then its program, then the program it starts, and by NAME only when the
dock already shows it as pinned (a second build of the same app with the same name is not "pinned"). Not yet done: dragging to reorder
or to pin.

### The hover previews (setting "Simge üzerinde pencere önizlemesi", on by default for the dock; the dock itself is opt-in)

The pointer resting on a RUNNING app's icon for 400 ms opens a small glass preview above it: one card per window of the app (up to 6
to a row and 3 rows; a window beyond that is not shown), each with a LIVE picture of the window (DWM, `DwmRegisterThumbnail`, through
the switcher's `ThumbApi`, this pop-up as the destination; registered after the glass is on, unregistered before the window moves,
changes size or hides), the app's large icon in the box for a MINIMISED window (or one DWM refuses), the title under the picture, and
on the card under the pointer a small x at the right end of the title row (not over the picture: DWM's picture is drawn over anything of
ours that lies on it). A click on a card brings that window to the front with the switcher's activation sequence
(`rf_win::switcher::activate`, off the UI thread: it may wait a quarter of a second for a minimised window to restore; it flashes
the window's button if the program refuses the foreground); the x asks the window to close.

* THE TIMING is `rf_win::dock::preview::Machine` (pure, tested): 400 ms dwell; moving to another running app's icon swaps the preview
  at once; moving from the icon into the preview keeps it open (either order of the two hover events); leaving both closes it after
  300 ms and coming back inside that time cancels the closing; a click on an icon closes it and it does not come back until the
  pointer has left that icon; a right click replaces it with the menu (the menu blocks the machine while it is open); a block closes at
  once. No polling: the app sleeps until the machine's next deadline.
* IT IS NEVER ACTIVATED AND NEVER TAKES THE KEYBOARD. The panels are activated (they need keys); this is not a panel. Its window is
  styled like the islands' (`island::make_island`: a tool window with `WS_EX_NOACTIVATE`, `MA_NOACTIVATE` for `WM_MOUSEACTIVATE`, so
  even a click on a card does not activate it), shown with `SWP_NOACTIVATE` and never brought to the front. A click on a card is still
  input of this process, which is what lets the activation sequence bring the chosen window forward.
* It closes when the dock hides for a full-screen application, when the monitor or the work area changes, when the setting is switched
  off, and when its last window closes; while it is open one 100 ms look removes the cards of windows that have closed and reads the
  REAL POINTER: Windows sends a window no last mouse move when the pointer leaves it, so the toolkit's hover events never report a
  pointer that goes from a card straight out of the preview or from an icon up out of the dock's window. When the pointer is on
  neither the preview nor the dock's glass (`preview::pointer_away`; the part an icon grows over the glass counts as the dock) the look
  tells the machine that it left both, and the 300 ms of grace runs from there. Expected limit (from reading the toolkit; not yet seen live): after that, coming back onto the very
  same icon gives no hover event either (the toolkit still believes the pointer is on it), so the preview comes again after the pointer
  has touched another element of the dock. There is no easing (DWM draws its pictures at full strength, so a fading card would show
  its picture first).
* LOG (info): `dock: hover preview shown in <time> at x=<device px> (<n> cards, <m> live pictures)`.

## The tray strip and its overflow panel (shell plan increment D3)

The other programs' notification icons in the right island: a row of small pictures to the LEFT of the island's own symbols, a chevron
that opens an overflow panel above the island, and a thin separator before the network symbol. **Off unless there is a tray source:** the
dock makes no tray state at all without one, and with a source that has no snapshot (the tray host off) the island is exactly what it
was. In this checkout the tray host (`rf_win::trayhost`, built by another team) does not exist, so the only source is the made-up one
(`RUSTYFILES_TRAY_FAKE=1`, below); the lead wires the real one in the five bodies of `tray_source.rs` that say WIRE AT MERGE.

```
 [ app app app app app app > ] |  [ net ] [ vol ] | [ 14:05 12.11.2026 ] [ bell ]
   strip: 16 px pictures in 24 px cells, chevron      the island's own symbols, unchanged
```

* **The strip.** Cells of 24 px (the picture 16 px, centred) at the island's left padding, as many as fit a width limit (a quarter of the
  monitor's work area, at most 240 px, chevron included), then the chevron. The icons that do not fit are the LAST of the strip and go to
  the overflow panel, together with the ones the user moved there. The chevron is there whenever there is any icon (it is the place to
  drop an icon to hide it; with nothing hidden the panel says so). The island is anchored at the right edge of the screen, so it grows to
  the left by `Layout::extra_width` and the symbols stay where they were; the dock's layout follows (`RightIsland::set_tray_extra` ->
  `host::set_width`), the middle island gives way as it always does.
* **Clicks.** Left, right, middle and the second press of a double click go to the program through the tray host's click engine
  (`tray_source::deliver_click(key, TrayClick { kind, at, id })`). A press is not yet a click: it might become a drag, so the click is made
  when the button comes up without the pointer having moved 4 px, and only if the pointer is still on the cell it went down on and the
  key still names the icon that was pressed. The click carries that icon's identity (`id`) and the host must refuse it when its icon
  under the key has another identity by then (a program that exited and another that took its key).
* **Tooltip.** The pointer resting on an icon for 400 ms opens its tip (cleaned: control characters and runs of blanks gone, cut at 200
  characters; no tip, no tooltip) in a small glass window above it that is NEVER ACTIVATED (the hover previews' kind of window). Moving to
  another icon swaps it at once, a press (which also covers the menu the program opens from the click), a drag or the keyboard in the strip closes it and keeps it
  from coming back until the pointer has left the icon, or the keyboard has left the strip. The window is made when the first tip is due (`tray_tip.rs`); the timing is `rf_win::dock::tray_tip` (pure).
* **Drag.** A left press that moves 4 px is a drag of that icon. The real pointer is read every 16 ms while the button is held (the
  toolkit's events stop at the window's edge and do not follow a drag from the overflow panel to the island). A marker shows the gap in the
  strip the icon would land on, the chevron lights when it is the target, the open panel gets a frame. Let go on a gap: reorder. On the
  chevron or the open panel: hidden (moved into the overflow). From the panel onto a gap of the strip: shown again. Anywhere else, or on
  the panel again for an icon that came from there: nothing happens. There is no picture of the icon following the pointer (the dragged
  icon is dimmed where it was).
* **Keyboard.** The islands are never activated, so the keys reach the strip only after `dock::focus_tray_strip(cx)` took the foreground
  for the island's window (`SetForegroundWindow` only: unlike the panels it does not touch the window's topmost state). Then: Left,
  Right, Home, End and Tab move a ring over the icons and the chevron, Enter or Space is a left click, the context-menu key or Shift+F10
  a right click (the click carries the icon's top left corner as its place), Tab past either end and Escape leave. The ring is on an icon by its key, so an icon
  that comes or goes under it moves the ring along and Enter always reaches the icon the ring shows. The keyboard goes back to the
  window that had it: at once after Escape or Tab; after a click 400 ms later, and only if the program did not take the foreground
  itself (an icon that does its work in the background); and Enter on the chevron opens the overflow panel with that window handed over
  as the one to go back to, so Escape in the panel gives the keyboard back to the user's program and not to the island. If Windows refuses the foreground the strip gives the keyboard up after 1.2 s. **Nothing calls
  `focus_tray_strip` by itself:** a key is wired to it at the merge. The overflow panel IS activated like the other panels and has its
  own keys: the arrows walk the cells (down into a shorter last row lands on its last cell), Tab and Shift+Tab walk on and wrap, Enter,
  Space, the context-menu key and Shift+F10 as above, Escape closes.
* **The overflow panel** (`tray_overflow.rs`, its slot `tray_panel.rs` is a child module of `panel.rs`): up to six columns of 40 px cells
  with 20 px pictures, lined up with the island's right edge like the calendar and the bell's panel (not above the chevron), made when
  first opened like the other panels, a tip after 400 ms, a click puts it away after the click was delivered (on every path, also when
  the icon went meanwhile). A drop on the end gap of a full strip makes the icon the strip's last cell (the last shown icon goes out).
* **The order** is `tray-order.json` in the config folder (`rf_win::dock::tray_order`): one list of every icon ever shown in the user's
  order and the list of the hidden ones. An icon is known by its GUID (when the program gave one) or by its program's path and the id
  the program chose, NEVER by its tip text (it changes with the program's state) and never by a window handle. An icon that is not there
  now keeps its place and its hiding; the list is cut at 256. The file is written (whole, to a temporary file, then renamed) only when the
  user changes something, never for reading; a file that exists and cannot be read is moved aside (`.broken`) before anything is written.
  A packaged app gets a new GUID with every version, so after an update its icon is a new icon at the end of the strip.
* **Where the icons are** is reported to the tray host (`tray_source::report_icon_rects`): the cells of the strip, and for an icon in the
  overflow its cell while the panel is open and the chevron's rectangle otherwise, for `Shell_NotifyIconGetRect`.

### The made-up source (`RUSTYFILES_TRAY_FAKE=1`)

Six invented icons (coloured 16 px squares): "Posta" (a GUID; a left click or Enter on it counts a message, which changes its tip and its
colour: the way an update from the host reaches the strip), "Bulut eşitleme", "Ses karıştırıcı", "VPN" (a GUID; a tip of two lines),
"Güncelleme" and one with no tip (a GUID; no tooltip). Three are known by a GUID and three by a program and an id. Clicks are logged
(`tray fake: click #n Kind on icon k at (x, y)`), nothing else. `RUSTYFILES_TRAY_FAKE_FOCUS_MS=<n>` moves the keyboard to the strip `n` ms
after the dock started, so that the keyboard can be tried before a key is wired to do it. Use a test config folder
(`RUSTYFILES_CONFIG_DIR`): reordering writes `tray-order.json` there.

`cargo run -p rf-win --example tray_strip_probe --offline -- <icons> <work width> <scale> <hidden>` prints where the cells, the chevron,
the overflow panel's cells and a tip fall for that many icons (arithmetic only; no window, nothing read from the system).

### Not run, and limits

* No window was shown by the work that wrote this: the strip, the drag, the tooltip, the overflow panel, the keyboard's taking of the
  foreground for a no-activate island and the keyboard in it have been compiled and covered by tests of the arithmetic and the state, not
  seen.
* The tray host is not wired; the wire format its parser expects is still the documents' and the other shells' (not this machine's).
* Not built: moving an icon by keyboard, a picture following the pointer in a drag, "restart that app" for programs that never re-add
  their icon, balloons, a Settings switch for the strip (the tray host being on is the switch). The island's own symbols (network, volume,
  clock, bell) are unchanged.

## Settings (Settings, "Dock (deneysel)" / "Dock (experimental)")

* **rustyfiles takvimini kullan** / Use rustyfiles' calendar. On by default. The clock opens the calendar panel; off: it sends Win+N
  (Windows' notification centre and calendar), as before.
* **rustyfiles bildirim panelini kullan** / Use rustyfiles' notification panel. On by default. The bell opens the notification panel
  and wears a dot while there are notifications; off: it sends Win+N, as before, and the dot's worker and folder watch are not running.
* **rustyfiles Başlat menüsünü kullan** / Use the rustyfiles Start menu. On by default. The logo opens the Start panel and the search
  field the search; off: the logo sends the Windows key and the field opens the quick search window, as in version 1.
* **rustyfiles denetim merkezini kullan** / Use the rustyfiles control centre. On by default. The network and volume symbols open the
  control centre; off: they send Win+A (Windows' quick settings), as in version 1. (The symbols themselves keep showing the real state
  either way.)
* **Dock'u göster (deneysel)** / Show the dock (experimental). Off by default. Switching it on starts the process now and at every
  start of rustyfiles; switching it off stops it. The dock keeps running when rustyfiles' window closes: it is your taskbar.
* **Pencereler dock'un altına girmesin** / Keep windows from going under the dock. On by default. The dock registers as an
  application desktop toolbar at the bottom of the primary monitor (the middle island's height plus the margins), so maximised
  windows stop above it, and unregisters on exit. Off: it only floats.
* **Simge büyütme** / Icon magnification. Full by default (see "Magnification").
* **Simge üzerinde pencere önizlemesi** / Window previews on icons. On by default (`dock_hover_previews`). Off: resting the pointer on
  an icon shows nothing.
* **Simge sağ tık menüsü** / Right-click menu on icons. On by default (`dock_app_menu`). Off: a right click on an icon does nothing, as
  before.
* **Hava durumu şehri** / Weather city. Empty (the default): no weather is shown and **no network request is made at all**. With a
  city, the dock asks Open-Meteo (`geocoding-api.open-meteo.com` once for the place, `api.open-meteo.com` for the temperature) every
  30 minutes. Nothing else leaves the machine: only the city name you typed and the coordinates it returned; no location API is
  used. A failed request shows nothing and is retried after 5 minutes, then 10, 20 and 30; a city Open-Meteo does not know is asked
  about again only once an hour.

The dock process is told to read the settings file again when any of these changes: the four switches of the panels (`dock_own_start`,
`dock_own_control_centre`, `dock_own_calendar`, `dock_own_notifications`), the two switches of the icons' menu and previews
(`dock_app_menu`, `dock_hover_previews`), the keep-out switch, the city, the magnification, the animations setting and the four HyperGlass sliders
(`control::Keys`). The process's views read the two panel switches from the settings it loaded last, at every click.

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
* What the glass engine costs (`crates/rf-win/src/glass`, shared with the main window and the popups; measured windowless with
  `examples/glass_session_cost.rs`, `glass_cover_probe.rs`, `glass_capture_check.rs`):
  * Every D3D11 device of a process has a thread of the graphics driver (`nvwgf2umx.dll` here) that wakes about 100 times a second
    from the moment the device exists, with no capture and no frame: the engine's device and GPUI's device each have one. It is the
    driver's, and cannot be changed from here.
  * A capture session is asked for at most 60 frames a second, 30 on battery or with the battery saver on (`wgc::min_interval_ticks`;
    never faster than the display makes them), where it was one refresh period (5 ms at 200 Hz). Nothing the engine animates itself
    needs a fresher backdrop: lenses and shapes draw again from the backdrop already held, and a window that is dragged is drawn on a
    picture that is at most one interval old. What is given up: a video, a game or an animation BEHIND a glass window shows through
    the frost at 60 instead of up to 200 updates a second. A running session takes a new interval (`SetMinUpdateInterval` on the live
    session; read back as 16.67 and 33.33 ms); the power state is looked at every 5 s. A frame that the engine asks for (the list is
    sent again: after a pause, `Exclusion::nudge`) is not held back by the interval (measured: 300 forced frames a second at 5, 16.7
    and 100 ms alike). NOT measured here: that natural frames follow the interval (this desktop made 0.3 to 7 frames a second in every
    run, far below either cap), and that the frame that ends a burst of changes is still delivered once the interval has passed (the
    documentation of `MinUpdateInterval` says nothing about it). `RUSTYFILES_GLASS_MAX_FPS=0` (or any number) sets the cap for one run
    of the program, 0 being the old behaviour: it is how to compare on a live desktop with a video playing.
  * Glass that is seen is the only glass that is worked for. A window that is hidden, minimised or cloaked, or that other windows hide
    completely (`cover.rs`: a z order walk from the top down to the window, counting only opaque windows), is not prepared, not
    drawn and not read back. The walk is made when the foreground window changes or moves, and otherwise every 100 ms while some
    glass is covered and every 500 ms while none is (the dock's islands are always in view, so they do not set the pace). A layered
    window without a redirection bitmap (a Chromium browser, and also a transparent overlay made with the same toolkit) counts only
    when it is maximised or has a title bar and is not a tool window. A glass with no area (an island that has not had its shape yet)
    is never covered. The window keeps reporting `Running`, and the app's Mica fallback is never involved (the app reacts to
    `Failed`, and to `Starting` only before the first picture). When it is in view again its capture is resumed first (the frames
    queued from before the pause are not drawn from: measured, a session left alone for 3 s hands out a frame from before the pause
    right after, and on a still desktop nothing newer follows; now the list is sent again, which makes the service produce a new
    frame in about 2 ms), then it is drawn from a fresh backdrop and presented whatever the loop guard remembers; meanwhile its window
    shows its last picture. The dock's islands are topmost and are only covered by other topmost windows; the full screen case stays
    the dock's own (`fullscreen.rs` hides its windows).
  * A monitor's capture session (frame pool and mirror: about 38 MiB of video memory at 2560x1440 here) is kept for 5 s after its last
    seen glass went, and then closed; it is opened again when a glass needs it. If that fails after sessions had worked in this run,
    it is tried again after 1, 2 and 5 s (the window keeps its last picture) before the process goes over to Desktop Duplication.
  * Every frame that is taken is copied whole into the mirror. Copying only the parts some glass reads was built and measured and taken
    out again: it saved 0.8 to 7 us of GPU time a frame against 15 us for the monitor (`glass_session_cost copy`), nothing on the CPU
    (205 us a frame with it, 214 without, in a forced-frame loop), and it made the mirror depend on the dirty rectangles of every
    frame being true across the frames a minimum interval skips, with a "wait and ask for a frame" path for a glass that moves onto a
    part that was not copied. Not worth a picture that can be wrong. (The dirty rectangles still decide which windows are drawn again;
    `glass_session_cost dirty` compares every picture with the one before it: 0 changed pixels outside the reported rectangles in
    30 s at the 16.7 ms interval, on a desktop that changed in only one picture, so this is weak evidence; the earlier probe at 400 ms
    saw 54 frames and none outside.)
  * Only a picture that changed something a glass reads counts as activity for the engine's clock. A frame that changed something
    elsewhere (a caret, a terminal) no longer keeps the loop at its 8 ms wait for a second after it, which on an ordinary desktop made
    it turn about 100 times a second instead of 20.
  * Taking a popup out of the exclusion list sends nothing: the removal goes with the next list that has to be sent. The handles that
    left are looked at once a second and the list is sent if one has become a window of another process. Opening a popup is two sends.
  * Where the busy threads of a process with glass are (no window needed to see it: `glass_session_cost run --drive 2`): the driver's
    worker (`nvwgf2umx.dll`, unnamed) wakes 2 to 4 times per captured frame and the capture service's thread once per frame, so a
    busy monitor at 200 frames a second is 400 to 800 context switches a second in threads that are not ours; the cap cuts that to
    a third of it when frames are the cause. The driver's timer thread (about 100 a second) is there with a device and no frame.
    A session's video memory is the same with one pool buffer as with two (48.5 MiB both), so the pool stays at two.
* `RUSTYFILES_NO_DOCK=1` keeps a rustyfiles from starting or stopping a dock (automated runs, a second copy for testing).
* Updates and uninstalling: the dock holds `rustyfiles.exe` open, so the updater's restart and the uninstall hook ask it to stop
  first; the next start of rustyfiles brings it back while the setting is on.
* Ending: `stop`, or Windows ending the session. The glass is taken off every island, the windows close, the toolbar band is given
  back. If the dock panics, the band is given back from the panic hook before the process aborts. If the dock is killed (Task
  Manager) nothing can run: the band stays registered until Windows notices the dead window (this has not been verified), and the
  dock's next start registers again. If Explorer restarts, the dock hears the `TaskbarCreated` broadcast and registers again.
* Keystrokes (`SendInput`: Win, Win+A, Win+N, and the Alt tap that lets a window come forward) are sent only as the direct answer to
  your click on that element, never at start-up and never from a timer. Win+A is now only the answer to the network and volume
  symbols with "rustyfiles denetim merkezini kullan" off; Win+N is the clock's and the bell's only with "rustyfiles takvimini kullan"
  and "rustyfiles bildirim panelini kullan" off.

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
  `icons.rs` (the icon cache, keyed by an app's first source), `actions.rs`. The panels:
  `panel.rs` (the hidden window, opening, closing, the watch), `start.rs` (the Start panel's view), `search.rs` (the result rows, the
  selection that stays on its row, whether Enter waits). `crate::search_pipe` is the pipe protocol for files; `quick_search.rs` holds
  the sources and the ranking.
* The control centre: `crates/rf-app/src/dock/control_centre.rs` (the view), `cc_model.rs` (what is shown, the heights, the words and
  symbols, the slider's pointer arithmetic; all pure), `sysstate.rs` (the sound and network the island follows all the time), and in
  rf-win `crates/rf-win/src/sysstatus/` (`audio`, `network`, `media`, `bluetooth`, `power`, `brightness`, and the hub / write-coalescing
  helpers in `mod.rs`).
* The tray strip: `crates/rf-app/src/dock/tray_source.rs` (the one seam to the tray host, and the made-up source), `tray_strip.rs` (state,
  pointer, drag, keyboard, the strip's elements), `tray_tip.rs` (the tooltip window), `tray_overflow.rs` (the overflow panel's view),
  `tray_panel.rs` (its slot in `panel.rs`); in rf-win `crates/rf-win/src/dock/tray_order.rs` (identity, order, file), `tray_strip.rs`
  (layout, drop targets, clicks, keys), `tray_tip.rs` (tip timing, text, window).
* The calendar and the notification panel: `crates/rf-app/src/dock/cal_view.rs` (the calendar's view and its small model),
  `notif_view.rs` (the notification panel's view), `notif_model.rs` (its height, words and cursor arithmetic; pure), `notif_state.rs`
  (the count behind the bell's dot and its worker), `panel_more.rs` (their slots in the panel machinery), `panel_parts.rs` (the colours
  and buttons they share); in rf-win `crates/rf-win/src/dock/calendar.rs` (dates, grids, names, keys) and `crates/rf-win/src/toasts/`
  (`mod.rs` the pure model, `listener.rs` the WinRT reader, `hint.rs` the dot's worker; named `toasts` because `notify` is a crate this
  one uses).
* The panels' system side in `crates/rf-win/src/dock/`: `panel.rs` (placement, toggle, close and foreground decisions, the easing),
  `power.rs` (the power menu and its question, the real calls, the user's name), `startlist.rs` (the alphabet, headers, rows, arrow
  keys), `startpins.rs` (the pin file and its seeding), `startapps.rs` (start, start as administrator, find the program).
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

## Not in the control centre (yet)

Switching Wi-Fi or Bluetooth on and off, choosing the output device (the list is read-only), per-application volume, the microphone,
airplane mode, hotspot, Night light, Focus, projection, HDR, the power-mode slider; brightness for a laptop's own panel (WMI); a
choice between several media sessions and a progress bar; the clock's and the bell's own panels (Win+N still opens Windows'); the
volume and brightness on-screen display for the hardware keys.

## Not in version 1

Hiding or replacing Windows' taskbar, becoming the shell, other monitors' docks, drag to reorder or pin, the jump-list DATA in the app
menu (the menu and its seam are there, see below), notification badges, progress rings. Hiding Explorer's taskbar is a later version and needs a watchdog that brings it back if the dock dies.

## Known limits

* Whether the islands' GLASS stands where the layout says: `visible_glass_rect` is the layout's glass rectangle, and the islands' windows
  are placed by their outer rectangle while the glass is drawn from the client area (which starts at the invisible frame). If the
  frame is really 8 px there, the seen glass is that far from the layout's, and a panel lined up with the layout's rectangle would be
  that far from the seen glass. One look at a running dock settles it (the first step of the panels' live test).
* What has been seen on a screen: the placement, the glass, the apps and their icons, the light or dark foreground (main's live runs,
  before the magnification was added). What has not: the magnification, the glass as a shape inside a larger window, the window
  region and the clicks through it, the merged whole, and the control centre and the Start panel (the sources were read with a
  windowless probe; the panels' look, placement against the glass rectangle, glass, tone, focus, dragging and the first-open growth
  are untested live). They are covered by pure tests of the arithmetic and decisions and by offscreen pictures of the glass. To look
  at: whether the shadow shows with `RUSTYFILES_DOCK_SHADOW=1` (expected not: see above), the glass following the icons frame by
  frame, the top of a grown icon while it is still growing (the region follows the scene by one task of the message loop and is a
  pixel larger than it), the corners of the region (a region has no anti aliasing; the glass's own edge is inside it), and clicks on
  the margin reaching the taskbar.
* On the first opening of the control centre the Bluetooth tile and the brightness row arrive a moment after the window is shown
  (their first reading takes tens to hundreds of milliseconds), so the panel grows upwards once; later openings start from what was
  last known. The brightness row sits between the sound card and the gear, so on that first opening it moves the sound card up by its
  own height (the tiles and the media card are above everything and move nothing).
* The icon cache of the Start panel lets go of the pictures of found files when the panel hides, but a fast scroll through the app
  list still queues one icon job per row passed (no cancellation); they share the shell thread with the middle island's icons.
* The 50 ms look at the foreground and the mouse buttons while a panel is open is kept (the foreground rule catches a click that took
  another window's foreground, which is almost every click outside). A press and release inside one 50 ms look that lands on something
  that does not take the foreground is the case it can miss.
* Wi-Fi name and bars could not be exercised here (this machine is on Ethernet).
* The pinned apps' order comes from the `Taskband` registry value read as text runs, not as a full shell item list; a Windows build
  that changes the record layout gives the shortcuts in name order instead.
* A frameless maximised window is told from a full-screen game by its sizing frame and maximised state; a game that is maximised and
  has a sizing frame would not hide the dock.
