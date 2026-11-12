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
| Middle (72 px high at rest) | the apps pinned to the Windows taskbar in its order, the other running apps, a thin separator, the Recycle Bin, Downloads | not running: start it; running: bring its last used window forward; in front: minimise it; middle click: another instance |
| Right (48 px high) | network and volume glyphs that show the real state (the volume glyph shows mute), the time and date in two lines, a bell | network, volume: our own control centre (see "The control centre"; with "rustyfiles denetim merkezini kullan" off, Windows' quick settings, Win+A, as in version 1); time, bell: Windows' notification centre and calendar (Win+N) FOR NOW: they are still Windows' own panels |

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
panel's bottom edge is 966 (8 px above the glass), not 962. The same rectangle is what the open panel's outside-click test keeps (a
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

**What it costs.** Each panel's window (the Start panel's and the control centre's) is made once, when the dock starts, and kept
hidden: two GPUI windows and their views at rest (memory not measured), no timer, no thread and no glass of theirs (the control
centre's sources run only while it is open; the sound and the network are followed all the time, as the right island shows them). The glass engine is attached when the panel opens and taken off after it has closed.
While it is open one 50 ms task looks at the foreground and the mouse buttons. Four seconds after the dock is up the app list, the
pins and the user's name are read once on the shell thread (about 0.4 s for 273 apps on the machine this was written on).

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

## Settings (Settings, "Dock (deneysel)" / "Dock (experimental)")

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
* **Hava durumu şehri** / Weather city. Empty (the default): no weather is shown and **no network request is made at all**. With a
  city, the dock asks Open-Meteo (`geocoding-api.open-meteo.com` once for the place, `api.open-meteo.com` for the temperature) every
  30 minutes. Nothing else leaves the machine: only the city name you typed and the coordinates it returned; no location API is
  used. A failed request shows nothing and is retried after 5 minutes, then 10, 20 and 30; a city Open-Meteo does not know is asked
  about again only once an hour.

The dock process is told to read the settings file again when any of these changes: the two switches of the panels (`dock_own_start`,
`dock_own_control_centre`), the keep-out switch, the city, the magnification, the animations setting and the four HyperGlass sliders
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
* `RUSTYFILES_NO_DOCK=1` keeps a rustyfiles from starting or stopping a dock (automated runs, a second copy for testing).
* Updates and uninstalling: the dock holds `rustyfiles.exe` open, so the updater's restart and the uninstall hook ask it to stop
  first; the next start of rustyfiles brings it back while the setting is on.
* Ending: `stop`, or Windows ending the session. The glass is taken off every island, the windows close, the toolbar band is given
  back. If the dock panics, the band is given back from the panic hook before the process aborts. If the dock is killed (Task
  Manager) nothing can run: the band stays registered until Windows notices the dead window (this has not been verified), and the
  dock's next start registers again. If Explorer restarts, the dock hears the `TaskbarCreated` broadcast and registers again.
* Keystrokes (`SendInput`: Win, Win+A, Win+N, and the Alt tap that lets a window come forward) are sent only as the direct answer to
  your click on that element, never at start-up and never from a timer. Win+A is now only the answer to the network and volume
  symbols with "rustyfiles denetim merkezini kullan" off; Win+N is the clock's and the bell's.

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

Hiding or replacing Windows' taskbar, becoming the shell, other monitors' docks, drag to reorder or pin, jump lists, notification
badges, thumbnails on hover. Hiding Explorer's taskbar is a later version and needs a watchdog that brings it back if the dock dies.

## Known limits

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
