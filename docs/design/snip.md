# Our own screen snip

Win+Shift+S is answered by Explorer, so without Explorer there was no screen snip ("ekran alıntısı da çalışmıyor sanırım ... gerekiyorsa
ekran alıntısını da biz yazalım rust ile"). This is ours. Status: built and tested without a screen; the overlay itself has NOT been seen
on a monitor (the brief forbids showing a full-screen overlay while the user works).

## Starting it

| How | Where |
|---|---|
| Win+Shift+S | the key layer (`shell/keylayer.rs`, `rf_win::winkey`), under the existing `shell_win_key` switch and ONLY while Explorer is not the shell (`combos_wanted`); beside Explorer the chord is Explorer's. See `shell-keys.md`. |
| the button "Ekran alıntısı" | the bottom row of the dock's control centre, left of the gear (`dock/control_centre.rs`). The panel is put away first and the snip starts 350 ms later so that the panel is not in the picture. |
| `rustyfiles.exe --snip` | a process that does only this and ends when it is over (no window of the app, no forwarding to a running copy, no dock, no key layer). |

## What happens

1. ONE capture, off the UI thread (`rf_win::snip::capture`): every monitor into memory at its own physical size, then the list of windows
   with the rectangles the user sees. **GDI `BitBlt` with `CAPTUREBLT`, not Desktop Duplication or a capture session.** The glass engine
   captures continuously and needs a device, a duplication or session per monitor kept alive and a wait for the first frame; duplication
   also allows only a few per desktop and the engine may hold them when the key is pressed. A snip needs one picture of each monitor at
   one moment: a `BitBlt` into a DIB section is that, needs no device and no first-frame wait, and cannot fail because someone else
   holds a duplication. Its limits are GDI capture's own: a window excluded from capture (`WDA_EXCLUDEFROMCAPTURE`, which the glass
   engine sets on its windows in the Desktop Duplication path) is left out, a protected video surface is black, an HDR monitor is its SDR
   rendering. Measured here (2 monitors, 2560x1440 and 1440x2560, both 100 %): 134 ms for 28 MB.
2. One borderless topmost window per monitor (`snip/overlay.rs`) shows that monitor's frozen picture, dimmed; the pointer is a cross; a
   drag makes a rectangle in which the picture is undimmed (the dim is four strips around it, `plan::dim_strips`), outlined in the system
   accent, with its size in pixels beside it. The pictures live once, inside the textures the windows draw; the cut reads the same bytes.
3. A small toolbar at the top centre of the monitor the pointer was on (`snip/toolbar.rs`, HyperGlass through `PopupGlass`, the
   dock's panel fill until the glass runs): **Bölge** (default), **Pencere** (the window under the pointer is outlined, a click takes it),
   **Tam ekran** (the monitor under the pointer), and a close button.
4. Release (or the click in the window and screen modes): the windows close at once; a thread cuts the region, makes `CF_DIB`,
   `CF_DIBV5` and `PNG`, puts all three on the clipboard and, with the setting, writes the PNG (`snip/finish.rs`).
5. Escape or a right click cancels. A click that is not a drag (less than 4 device pixels on both axes) takes nothing and the overlay stays.
6. The notice (`snip/notice.rs`, the dock's tone): "Panoya kopyalandı" and **Kaydet**, which writes the PNG to
   `Pictures\Screenshots` (made if missing) as `Ekran alıntısı YYYY-AA-GG SSddss.png` (never overwriting: a second snip in the same
   second is `... (2).png`). It stays 6 s unless the pointer is on it or a save is going on. When the clipboard could not be written it
   says so and still offers Kaydet; when the setting saved the file it says "Panoya kopyalandı ve kaydedildi" and offers nothing.

The setting `snip_autosave` (Settings -> Genel, "Ekran alıntısı"; **off by default**) saves every snip to that folder without asking.

## Decisions worth knowing

* **Physical pixels everywhere.** The clipboard picture is a copy of the screen as drawn, so a region is in device pixels whatever the
  scale of the monitor. The overlay windows are in the toolkit's logical pixels; `geom::to_screen` is the one place that turns a pointer
  position back (with the scale and origin of the window the drag began in: the toolkit keeps delivering a held button's moves to the
  window that was pressed, `SetCapture`). A drag from a 150 % monitor to a 100 % one is one region (tested, negative origin included).
* **Nothing takes the foreground** (`WS_EX_NOACTIVATE` on every window, and the procedure put in front of each of them answers
  `WM_MOUSEACTIVATE` with `MA_NOACTIVATE` itself: the toolkit answers `MA_ACTIVATE` for every window, an explicit answer the style is not
  known to override, as the dock's islands and the desktop cover found before). A click on an activatable overlay would raise it over
  its own toolbar (both are topmost), and the program the user was in keeps the keyboard, so there is nothing to give back.
* **Escape is taken, not just seen.** The overlay has no keyboard, so Escape would also reach the program underneath (cancel its
  dialog, abandon its rename, leave its full-screen video). While the overlay is up `escape.rs` holds a low-level keyboard hook, on a
  thread of its own, for that one key: a fresh plain Escape is reported to the snip and swallowed with its repeats and its release;
  Escape with Ctrl, Alt or a Windows key, and an Escape already down when the overlay appeared (until it has been seen up once), pass
  untouched; every other key passes. The hook exists only for the overlay's lifetime. If it cannot be installed the key's state is
  looked at (`GetAsyncKeyState`, 30 ms) instead and the key does reach the other program (logged).
* **Nothing stays up by itself.** `plan::SessionWatch`, checked on the same 30 ms tick: another program in front (judged after a 400 ms
  settling time; a window of our own is not another program), a changed screen shape (virtual-screen rectangle and monitor count) or two
  minutes ends the overlay. A picture that has not come after 10 s is given up (`CAPTURING` is cleared). A session none of whose windows
  could be shown is closed at once. Every failure before the overlay stands (no picture, one monitor failed, a window not made) shows
  the notice "Alıntı alınamadı". A second Win+Shift+S while an overlay is up closes it and begins again.
* **`Kaydet` is work of the process.** The write counts in `WORKING` until it ends, so closing the notice of a `--snip` process during
  the write does not end the process under it.
* **CF_DIB is 24-bit BI_RGB, bottom-up**: the form the oldest readers want, and a 32-bit BI_RGB picture is read by some programs as
  all-transparent. `CF_DIBV5` is 32-bit with explicit masks, the sRGB colour space, alpha 255. `PNG` is the registered format named "PNG".
  All three are decoded again in the tests (`image` decodes the DIBs as BMP files and the PNG).
* **Clipboard owner**: `OpenClipboard(NULL)` + `EmptyClipboard` leaves no owner and `SetClipboardData` then fails, so the writer makes a
  message-only window on its worker thread for the time it needs. Data is handed over rendered (no delayed rendering).
* **Cost.** Off (nothing asked): none; the module is code only. On: one screenshot of every monitor in memory from the key to the end of
  the overlay (about 33 MB per 4K monitor), one 30 ms timer while the overlay is up, three short threads (capture, copy, optional save).
  A folder of 100 000 items is not touched.

## Files

`rf-win/src/snip/`: `geom.rs` (region from a drag, monitor and window under a point), `model.rs` (modes, drag, highlight, what a release
takes), `frame.rs` (the cut across monitors), `encode.rs` (DIB / DIBV5 / PNG), `save.rs` (name, folder, exclusive write), `clip.rs`
(call order over a trait; the real clipboard), `capture.rs` (GDI capture, window list and its filter), `place.rs` (overlay window
styles, placement, a guard that holds the rectangle, the answer to a click), `escape.rs` (Escape taken from the program underneath). `rf-win/src/winkey/` carries the chord. `rf-app/src/snip/`: `plan.rs`,
`finish.rs`, `overlay.rs`, `toolbar.rs`, `notice.rs`, `mod.rs`. Probe: `cargo run -p rf-win --example snip_probe`.

## Not run (the lead does it)

The overlay on a screen: its look, that a click on the overlay and on the toolbar leaves the foreground window as it was, that Escape reaches nothing underneath (try it over a dialog and over a full-screen video), that Alt+Tab ends the overlay, the cross cursor, a drag on one and on two monitors, the toolbar's glass, the notice, Escape, that a
window with `WS_EX_NOACTIVATE` receives the mouse events in this toolkit, that the drag's moves arrive beyond the window's edge, that the
placement keeps the client area exactly over the monitor, Win+Shift+S through the real hook, the control centre's button, the real
clipboard and a paste into other programs, `Kaydet` and the saved file in `Pictures\Screenshots`, mixed-scale monitors (this PC has
two monitors at 100 %).
