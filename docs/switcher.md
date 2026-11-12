# The window switcher (`rf_win::switcher`, `rustyfiles --switcher-preview`)

Windows' own Alt+Tab, Task View and Snap flyout live in `explorer.exe` and go with it, so a shell has to bring a switcher. This one
is a model (what is listed, in what order), a chord (the keys), a layout (where the cards go), a window (a glass panel with one card
per window and a live picture on each) and a short sequence that brings the chosen window to the front.

Beside Explorer it is tried with a command and Windows' Alt+Tab is left alone, unless the setting `shell_own_switcher` is on (off
by default): then the shell host's keyboard layer (`rf_win::winkey`, `docs/design/shell-keys.md`) feeds the chord from the one
keyboard hook. **Nothing registers a global hotkey, and the hook exists only in the shell host and only while that setting (or
`shell_win_key`) is on.** `--switcher-preview` installs no hook.

## Trying it

```
set RUSTYFILES_CONFIG_DIR=%TEMP%\rf-switcher-test
set RUSTYFILES_MULTI_INSTANCE=1
set RUSTYFILES_MUTE=1
set RUST_LOG=warn,rf_app::shell::switcher=info
rustyfiles.exe --switcher-preview
```

It opens once, as if Alt+Tab had been pressed (the card of the window used before the current one is selected; when the window in
front is not a card, the desktop or our dock for instance, the first card), on the monitor the pointer is on, and stays until a
window is chosen (Enter, a click), Escape, Alt+F4, a click outside it, or another window takes the foreground (also when it never
got the foreground itself: after the dock panels' 1.2 s grace). If no window can be made the process ends at once. It re-points no
registration (the registry healing the other previews do is skipped for this switch). Tab and
Shift+Tab step, the arrows move over the grid. Choosing a window brings it to the front (a minimized one is restored first); the
process ends when the switcher closes. The log line "switcher: opened in ..." says what the opening cost, by part.

## What is listed

The z-order walk with the rules of Windows' own switcher (`rules.rs`, tested with made-up window facts for every rule):

* each window's root owner, then the chain of last active popups: one entry per app, and the entry is the dialog an app has open;
  a tool palette used last does not take the app out of the list (a change to the classic walk);
* tool windows, windows that cannot be activated, zero-size helpers, the desktop and taskbar windows, a Store app's hosted
  `CoreWindow` (its frame is listed) and our own shell windows are never listed;
* windows cloaked by DWM: hidden by their own app = not listed; cloaked by the shell = another virtual desktop (left out by
  default, counted) or a suspended Store app on this one (kept, shown like a minimized window, but only when it IS minimized: a
  titled frame cloaked on this desktop and not minimized is the leftover of a closed app, with nothing to switch to; it is left out
  and counted), told apart by `IVirtualDesktopManager`; when it cannot say the window is left out;
* minimized windows are listed. The window in front is first; always-on-top windows come after the ordinary ones (the z-order
  puts them first whether or not they were used last, which would send the first Tab to a pinned note instead of back to the
  previous window).

**The order is NOT a true most-recently-used list.** It is the z-order, which follows use only for windows that are neither
minimized nor always-on-top: a minimized window sits at the bottom, where Windows put it when it was minimized, not where the user
last used it (Windows' own list puts it second). The plan's "ordering by last activation" needs a record of activations (a
foreground event hook running in the shell host); nothing in the switcher keeps state while it is closed, so that is the host's
job, later.

Choosing a window brings it to the front by the foreground rules, and a window that is "in front" but still cloaked after a short
wait (150 ms per try) is not counted as done: its button flashes instead.

## Cards and pictures

Up to about 12 windows fit at full size on an ordinary monitor (224 x 176 logical px per card, 2 rows of 6 on a 1920 x 1080 screen);
more scale the cards down in steps of 5 % to half size; past that the oldest windows are not shown. Rows are evened out and the last
row is centred. A live picture is `DwmRegisterThumbnail` with the switcher's own window as the destination, drawn at the layout's
rectangle for the card; it is registered when the window is on the screen and unregistered when it closes. A minimized window, a
suspended app and a window DWM refuses show the app's icon large instead.

## For the shell host (done in increment B7: `rf-app/src/shell/keylayer.rs`)

The host opens the switcher with `shell::switcher::open(OpenRequest { hooked: true, on_closed: Some(..), .. })` and feeds the
chord's effects to `apply_effect`. `Effect::Open` is already done by `open` and is ignored by an open session, so feeding it back
is harmless. `on_closed` is called once however the switcher closed (a card chosen, Escape, a click, the foreground lost) and is
where the host's own chord is told (`Chord::closed_elsewhere`); without it the chord would go on swallowing Tab, the arrows, Enter
and Escape for a switcher that is gone. With `hooked` only a click outside puts the window away on its own.

The keyboard hook (`hook.rs`, installed by `winkey::KeyService` only, see `docs/design/shell-keys.md`; its callback now runs both the
chord and the Windows-key machine, allocates nothing and takes no lock) knows Shift and Ctrl from the events it sees, not from `GetKeyState`
(which follows the messages a thread has read, and the hook thread reads none); finds the "windows of this app" key by its scan
code (0x29, the key above Tab on every layout); and treats a key pressed with Ctrl down as not its own, which keeps AltGr (a right
Alt plus a Ctrl Windows makes up) and Ctrl+Alt+Tab out of the chord.

## What costs what

Nothing exists while the switcher is closed: no window, no pictures, no timer, no thread, no hook. While it is open one task looks
at the foreground and the mouse every 50 ms (it ends with the window). Only the icon bitmaps of the last opening are kept in
memory (a few dozen small ones), by program or package and size; an icon that came from a window handle is not kept. Opening is: one walk of the top-level windows, one COM round trip only when a
shell-cloaked window exists, the creation of one GPUI pop-up window, and one DWM registration per live card. The first two and the
pure part were measured with `cargo run -p rf-win --example switcher_probe` (counts only, windowless); the window's creation and the
registrations need a window and are measured by the log line of a live run.

## What has been seen, and what has not

Run: every rule, the chord, the selection, the layout and the activation order with made-up facts and fakes; the listing, read-only,
on a real desktop (counts compared with the dock's task rules); the whole `rf-win` and `rf-app` test binaries; the app builds.

Not run (no window may be shown by the work that wrote this): the panel itself; GPUI's drawing of the cards; whether DWM's pictures
show over a GPUI window with the glass attached (the first thing to look at in a live run); the glass on this window; the keyboard
and the foreground in a real run; activation of a minimized, a suspended or an elevated window; the real keyboard hook; the Alt
release mask key. The real `WH_KEYBOARD_LL` hook, the real activation calls and the real DWM calls are only ever reached by the
user's own use of the switcher (with `shell_own_switcher` on, the hook; the mask key is `winkey::mask`).
