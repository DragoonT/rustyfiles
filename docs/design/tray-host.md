# The tray host (experimental, off by default)

Shell plan increment D2 (rows B3 to B6 and B9 of the plan's section 1.2). One setting, one switch on the dock's Settings page
("Dock (deneysel)"), OFF:

| Setting | What it does |
|---|---|
| `shell_tray_host` | While Explorer runs, the shell host owns a window of class `Shell_TrayWnd` IN FRONT of Explorer's, keeps a model of the icons that programs send to the notification area, and passes EVERY message on to Explorer's own tray window, unchanged, returning Explorer's answer. |

The dock does not draw the icons yet (the strip is increment D3). Nothing in this increment changes what the user sees. What exists is
the host, the model the strip will read (`shell::trayhost::view`), the click engine the strip will call, and the answers to the icon
position queries.

`rf_win::tray` (rustyfiles' own tray icon) is a different thing and is not touched. Its `Shell_NotifyIcon` calls go to whichever
`Shell_TrayWnd` is first, which with the host on is ours: they pass through the host to Explorer like everybody's.

## Where the pieces are

| File | Role |
|---|---|
| `rf-win/src/tray_wire/*` | The pure parser of what programs send (exists; the host builds on it and knows no offset). |
| `rf-win/src/trayhost/model.rs` | The model: icons, merge rules, generations, bounds. Pure. |
| `.../pixels.rs` | Colour / mask / monochrome arithmetic, shrinking to 64 x 64. Pure. |
| `.../core.rs` | The handler: parse, own handling, pass on untouched, answer as a tray when there is none. Over traits. |
| `.../relay.rs` | Explorer's tray window: find, cache, check before each send, look again, send. Over traits. |
| `.../clicks.rs` | Host events to the callback messages of the icon's version; dead owners. Over traits. |
| `.../lifecycle.rs` | Start (window, THEN broadcast), stop (window FIRST, then broadcast), the 250 ms tick. Over traits. |
| `.../order.rs` | Strip / overflow / order from the user's own list (row B9). Pure. |
| `.../shared.rs`, `view.rs` | The lock, the generation, the counters, the event hints, the host thread's heartbeat; what the interface holds. |
| `.../handshake.rs` | Pure: who closes the window when the starter gave up waiting (a late start leaves no window behind). |
| `.../stall.rs` | Pure: notices a host thread whose heartbeat stopped (a log line; it cannot repair one). |
| `.../traits.rs`, `fakes.rs` | The small traits and their stand-ins. |
| `.../win.rs` | The real implementations, the window class, the window procedure, the thread. NEVER run by a test. |
| `.../host.rs` | `TrayHost::start` / `stop`. |
| `rf-app/src/shell/trayhost.rs` | The setting, the environment variables, start / stop, the counts in the log. |
| `rf-app/src/dock/host.rs` | Three one-line hooks: `trayhost::on_settings` at start and at each settings reload, `trayhost::stop` in `shutdown`. |
| `rf-win/examples/trayhost_probe.rs` | Windowless, read-only probe (see the end). |

## What is taken from documents and NOT measured here

The wire format has never been captured on this Windows (build 26300, 26H2). The `tray_spy` example, which nobody has run, would
settle it. Everything below is from the research note `2-tray-notifications.md` (three open implementations, Geoff Chappell's pages,
the SDK header) and every number lives in `rf_win::tray_wire`, nowhere else:

* the signature `0x34753423` at offset 0 and `dwMessage` at 4 of a `dwData = 1` block; `NOTIFYICONDATAW` from offset 8, 956 bytes with
  32-bit handles (`hWnd` +4, `uID` +8, `uFlags` +0xC, `uCallbackMessage` +0x10, `hIcon` +0x14, `szTip[128]` +0x18, `dwState` +0x118,
  `dwStateMask` +0x11C, `szInfo[256]` +0x120, `uTimeout/uVersion` +0x320, `szInfoTitle[64]` +0x324, `dwInfoFlags` +0x3A4, `guidItem`
  +0x3A8, `hBalloonIcon` +0x3B8); a possible trailing 4-byte word (kept, never relied on);
* that shell32 rebuilds the block in the newest layout whatever size the program passed, so a 32-bit and a 64-bit sender send the same
  bytes (so the inner `cbSize` is not trusted; the block's `cbData` is);
* the `dwData` values 0 (appbar), 1 (icon), 2 (service object), 3 (icon rectangle), and the layouts of the appbar block (two known
  layouts of 0x38 and 0x40 bytes, padding unsure) and of the rectangle query (40 bytes, magic unverified);
* **how the two replies to `Shell_NotifyIconGetRect` are packed.** Two sources disagree (`MAKELONG(left,right)` / `MAKELONG(top,bottom)`
  against top-left / bottom-right). Therefore the host does NOT answer those queries by default: Explorer does (see below);
* that `Shell_NotifyIcon` finds the tray window with one `FindWindow` call per call (so that the first window of the class wins); one
  source says it broadcasts. The design works under both (our window first AND everything forwarded);
* the callback packing for versions 0, 3 and 4, the foreground rule (`AllowSetForegroundWindow` before a button-down), what Explorer
  sends for a click (down, up, `NIN_SELECT` from version 3; down, up, `WM_CONTEXTMENU` from version 3), and that Windows 11's own tray
  never sends `NIN_POPUPOPEN` (off here too);
* that a repeated `NIM_ADD` of an icon Explorer already has returns false and changes nothing (probe P7) and that programs which add
  again after `TaskbarCreated` cope with that;
* that programs read the property `TaskbandHWND` from the first `Shell_TrayWnd` they find (the property EXISTS on Explorer's window
  on this machine: the read-only probe lists it; that programs read it from whichever window is first is from the note, not seen).
  The probe also lists the 20 other property names Explorer's window has here (`TaskbarMonitor`, `TaskbarVisible`,
  `IsAutoHideEnabled`, `TaskbarDPI_*`, `LastAutoHideBarStuckMonitor`, but also `OleDropTargetInterface`, `OleEndPointID`, `MSAA_*`,
  `SysSetRedraw`, `WindowOnEdge:*`, `AllowConsentToStealFocus`). The host copies the eight that state a FACT about the taskbar
  (`lifecycle::should_mirror_prop`) onto its window, so that taskbar progress and overlays keep working for programs, and copies none
  of Explorer's own OLE / accessibility / docking state (a name it does not know is not copied: a missing fact is better than a wrong
  one). Every 4 s, so a value that Explorer changes (auto-hide) is up to 4 s late on our window;
* that a hidden `Shell_TrayWnd` is found by `FindWindow` and by the lookup in shell32 (the other implementations do it; a program that
  tests the window's visibility would be an exception: `RUSTYFILES_TRAY_VISIBLE=1` makes it visible and click-through);
* that shell32 waits about 4 s for the taskbar's answer (the forward limit below is set from it);
* **integrity levels (UIPI).** The window is made at the dock's own integrity level (medium) and no message filter is set on it
  (`ChangeWindowMessageFilterEx` is NOT called: widening who may send to a window that parses what it receives is a decision for
  after the first try, and whether `WM_COPYDATA` can be allowed at all is not checked here). Senders at a HIGHER level (elevated
  programs) can send to a medium window. Whether Explorer's own `Shell_TrayWnd` carries a filter that lets a LOWER-level sender
  through is not known here. If it does, such a sender's tray call goes to our window first and may be refused by the system before
  our code sees it, so its icon would fail while the host runs. Not measured; step 9 of the first try asks for a note of any
  program that runs at a low level.

If a made-up byte block is the only evidence for a layout, a test says "from the document". A parse that does not fit is a count
and the message is still passed on; nothing is dropped because the host did not understand it.

## The forwarding rule

For EVERY `WM_COPYDATA` that reaches the window (`core.rs`, `on_copydata`):

1. only when rectangle answers are switched on and this is an icon-rectangle query: try to answer it from the interface's rectangle
   (nothing else runs before the pass-on);
2. pass the UNTOUCHED message (same `wParam`, same `lParam`, so the same `COPYDATASTRUCT`) to Explorer's tray window with
   `SendMessageTimeoutW(SMTO_ABORTIFHUNG, 4000 ms)`; if the target is gone (Explorer restarted), look for the new window once and
   send once more;
3. own handling, inside `catch_unwind`: parse with `tray_wire` (a failure is a count); for `dwData = 1` read the icon's pixels (the
   sender is still blocked until we return, so its `HICON` is alive) and update the model under the lock for ONE update, taking
   Explorer's answer into account (below);
4. return what Explorer returned; if there was no tray window to pass it to, answer what a tray would (below).

THE ORDER IS THE POINT, and it differs from the first version of this host: Explorer gets the message BEFORE any code of ours that
could fail runs. The sequence in `core.rs` is: (1) only if rectangle answers are on and this is a rectangle query: try to answer it
from the interface's rectangle; (2) pass the message on and take Explorer's answer; (3) parse it, read the icon's pixels, look up
the owner, update the model; (4) answer. The icon's picture is still alive in step 3: the sender is blocked in its own
`SendMessage` until our window procedure returns, and only then may it destroy its `HICON`.

**Panics.** Step 3 sits inside `catch_unwind`, and so does the whole window procedure. That helps in the test and development
profiles ONLY: the release profile of this workspace sets `panic = "abort"`, where `catch_unwind` does nothing and a panic anywhere in
the handler ends the dock's process. The window goes with the process and Explorer's is first again; what the order buys is that
the message that was being handled has already reached Explorer. Nothing is promised beyond that: a panic in the host takes the dock
down. The code is written not to panic (checked indexing and arithmetic in the model and the pixel code; the parser is fuzzed), and
the `handler_panics` count exists for the unwinding profiles.

A test sends 6000 random and mutated blocks and checks that each one was passed on exactly once. Other tests check the order
(`nothing_of_ours_runs_before_explorer...`: at the moment the relay sees the message no counter of ours has moved; the pixels are
read after the pass-on).

**What Explorer's answer changes in the model.** The model must not drift from the list Explorer holds, so the answer is an input:
* `NIM_MODIFY` and `NIM_SETVERSION` that Explorer REFUSED are not applied (no picture is read either);
* `NIM_DELETE` is applied whatever Explorer said (a program that no longer wants its icon must not leave a ghost);
* `NIM_ADD` is applied whatever Explorer said, because Explorer refuses the add of an icon it already has and that is exactly what
  the start broadcast makes programs send: it is how the model fills. But an add of an icon the model already holds starts a new life
  for the icon (version back to 0) only when Explorer ACCEPTED it; a refused one leaves the version as it is, as Explorer does;
* with no answer (a timeout, a vanished window, no Explorer window) every request is applied.
`explorer_refused` counts the icon requests Explorer refused. **Known limit:** an icon the model meets for the first time through a
refused add (Explorer had it before the host started) is entered with version 0, and its real version is whatever the program set
earlier. A program that sends `NIM_SETVERSION` after every add (the documented pattern, and what most programs do) fixes it at once;
one that sends it only when the add succeeded leaves the version unknown to the model until it adds again after Explorer lost the
icon. Nothing can ask Explorer for an icon's version. The first try (step 4) compares `re_adds`, `set_versions` and
`explorer_refused` to see how common that is.

Passed on untouched, not looked at: `dwData` 0 (appbar, including the shared-memory handle that Explorer fills in the sender's
process), 2 (service objects), and anything unknown. Messages other than `WM_COPYDATA`: `WM_COMMAND`, `WM_ACTIVATEAPP` and the
`WM_USER` / `WM_APP` ranges are passed on (`WM_USER + 372` posted, the rest sent), as the other hosts do.

**What is NOT passed on, said plainly.** Every other message that reaches our window is handled by `DefWindowProc` of OUR window and
Explorer never sees it: the registered messages (`0xC000` and up; `TaskbarCreated` and the other broadcasts reach Explorer's window
by themselves, but a registered message that is SENT or POSTED to the first `Shell_TrayWnd` does not), and the system messages
(`WM_SYSCOMMAND`, which some tools post to the tray window to open Start, `WM_GETOBJECT` from accessibility clients, and so on).
While the host runs, a tool that addresses the first `Shell_TrayWnd` with such a message gets our window's default answer, which
does nothing. This is inherent to a window in front of Explorer's and forwarding them blindly would be wrong in other ways (a
`WM_NCHITTEST` or `WM_PAINT` for our window is not Explorer's). Two counters make it visible instead of silent:
`local_registered` (registered messages) and `local_sent_by_others` (any other message of this kind that another thread sent, as
`InSendMessage` tells; our own window's housekeeping comes from our thread and is not counted). Both are expected to stay near 0;
a rising number during the first try names a message class to look at. The messages are not logged individually.

**The answer when there is nobody to forward to** (Explorer's tray window is gone, or the host runs as the shell): `NIM_ADD` of a new
icon 1, of a known one 0 (the fields are merged all the same); `NIM_MODIFY` and `NIM_DELETE` 1 for a known icon, 0 for an unknown one;
`NIM_SETVERSION` 1 for 0, 3 and 4 on a known icon; `NIM_SETFOCUS` 1; everything else 0.

**`Shell_NotifyIconGetRect`** (`dwData = 3`): answered from the rectangle the interface reports for that icon
(`TrayView::set_icon_rects`) only when `RUSTYFILES_TRAY_RECT_PACKING=axes|corners` names a packing; otherwise, and for an icon the
interface does not show, and for a rectangle that does not fit 16 bits, the query is passed on to Explorer. The default is Explorer's
answer because a wrong packing puts a program's flyout in the wrong place. The calibration is probe P6: with the packing set, call
the real `Shell_NotifyIconGetRect` for a known rectangle, and keep the packing for which the rectangle comes back
(`tray_wire::rect::matching_packings`).

## Threads and locks

* The host has ONE thread with its own message loop and nothing else on it. It never touches GPUI, never calls into a window of the
  interface, never waits for the interface.
* The model sits behind one lock held for one update (a copy of a few fields). It is never held while waiting for Explorer, while
  reading an icon, or while sending to an owner. The interface reads a `Snapshot` (a clone; pictures are shared) or the generation
  number (no lock).
* The window procedure can be entered again while it waits for Explorer: another program's `WM_COPYDATA` is delivered to a thread that
  is inside `SendMessageTimeout`. Nothing is borrowed across that wait (the `Core` is behind an `Rc` that is cloned out of its cell,
  the relay's cache lock is released before the send). A test makes a second message arrive inside the relay.
* The whole window procedure is inside `catch_unwind`, which keeps a panic from crossing the `extern "system"` boundary in the
  unwinding profiles. In the release profile (`panic = "abort"`) it does nothing; see the forwarding rule.
* The 250 ms keeper asks `FindWindow("Shell_TrayWnd")` which window is first (the very call programs make; the window manager stops
  at the first match) and raises OUR window only when it is not. It no longer walks every top-level window. `raised` counts the
  raises: a number that keeps growing means Explorer re-asserts its own place and the keeper is fighting it (a finding for the
  first try; the cost at rest is NOT measured here).
* The dock's own `SHAppBarMessage` calls (it registers as a desktop toolbar) come from the interface thread, land in this window, and
  are passed on by the host thread; the host thread sends nothing to the interface thread.
* No COM is used anywhere in the tray host, so nothing goes through `rf_win::sta`. The window is made when the setting goes on
  (at the host's start or at the first settings reload with it on), which is fast (one `CreateWindowEx`); nothing is hidden-but-dirty,
  because it is never painted.
* The tick (250 ms, only while the host runs) keeps ours the first `Shell_TrayWnd` (`SetWindowPos(HWND_TOPMOST)` on OUR window only),
  copies Explorer's rectangles and properties onto ours every 4 s, and every 5 s sweeps icons whose owner windows are gone (see the
  deviations).

## The model

Per icon: key (GUID when `NIF_GUID` and non-zero, else window + id), owner window (raw 32 bits), id, callback message, version
(0, 3, 4), tip, state (`NIS_HIDDEN`, `NIS_SHAREDICON` merged under the mask), picture (BGRA, straight alpha, at most 64 x 64, copied in
the handler), the owner's process id and program file name (read at `NIM_ADD`), the latest balloon, and generation stamps. Bounds: 256
icons (a 257th `NIM_ADD` is not added; it is still passed on), pictures 64 x 64 (larger ones are averaged down; a source larger than
512 x 512 is not read), tips 256 characters. Only fields whose `NIF_*` flag is set are merged. `NIM_ADD` of a known icon merges and
resets the version to 0 when Explorer accepted the add (Windows forgets the version at every add; the program sends it again), and
keeps it when Explorer refused the add. `NIM_MODIFY` of an unknown icon is
counted and ignored (not an implicit add). A modify that changes nothing bumps no generation. Ids are never reused, so a click queued
for an icon that has gone cannot land on another.

Events (`TrayEvent`) are hints on a bounded queue (1024); a full queue drops hints; the generation is the truth.

## The click engine

`TrayView::click(ClickRequest { id, event, cursor, icon_top_left, opts })`, from the thread that received the input:

1. the icon is looked up (a copy of its owner window, id, callback, version, process);
2. a window that is not a window any more, or that belongs to another process than the one that added the icon, drops the icon from
   the model and NOTHING is sent;
3. for the events after which a program opens a menu or a window (clicks, double click, keyboard, balloon click),
   `AllowSetForegroundWindow(pid)` first;
4. the messages of `tray_wire::click::callbacks` for the icon's version, in order, by `SendNotifyMessage` (queued when the owner is on
   another thread, so a hung program cannot freeze the caller). A refused send (an elevated owner) stops the sequence and does not drop
   the icon.

## If the host is killed, hangs, or Explorer restarts

* **Killed** (Task Manager, a crash, a panic in a release build): the window goes with the process, and Explorer's `Shell_TrayWnd` is the first again. Because
  every message was also passed on, Explorer's own list is complete. Nothing broadcasts `TaskbarCreated` on a kill, so nothing is
  asked of the programs. If shell32 looks the window up on every call (what the other implementations assume), programs notice
  nothing beyond the messages that were in flight. **If some program had found our window and kept its handle**, its tray calls fail
  (the window is gone) until it looks again, is restarted, or Explorer restarts and broadcasts `TaskbarCreated`. That is not
  measured here.
* **Hung** (the host thread stuck in a wait for Explorer): senders wait up to their own limit (4 s) and then give up; Explorer's list
  misses the messages that were waiting. The forward uses `SMTO_ABORTIFHUNG`, so a hung Explorer returns at once. **There is no way
  to take the window away from a stuck thread:** a window can only be destroyed by the thread that owns it, and a stuck thread does
  not run. Every wait on the host thread is bounded (the forward 4 s; `AllowSetForegroundWindow` and `SendNotifyMessage` do not
  wait), so a stall is not expected, but if one happens every program's tray call fails after its own wait until the thread
  returns to its loop or the process ends. What exists is detection, not repair: the host thread beats a counter every 250 ms, the
  app's reporting thread watches it, and when it stands still for 15 s the log says `the host thread has not ticked for N s` (once),
  and `... ticks again` when it moves. `TrayHost::stop` also says (error level) when the thread did not end within 3 s and that its
  window stays until it does or the process ends. The remedy is to end the dock process.
* **A start that took too long** (more than 5 s to make the window): the starter reports an error, and tells the host through the
  handshake that nobody waits any more; a host that makes its window after that closes it again at once (its message loop ends and
  unwinds as always, with the `TaskbarCreated` broadcast), and one that has not begun makes none. A failed start leaves no window.
* **Stopped** (the setting off, the dock's exit, `Drop`): the window is destroyed FIRST, then `TaskbarCreated` is broadcast so that
  every program adds its icons again to Explorer's window (which is the first again). Programs that ignore `TaskbarCreated` keep their
  icons in Explorer's list anyway (they were forwarded).
* **Explorer restarts**: Explorer's new taskbar broadcasts `TaskbarCreated`; the host looks for the new tray window at the next message
  (a send to the old one that fails does it at once), keeps its model (programs add again and the model merges), and raises its
  window if Explorer's is on top. While there is no Explorer window it answers as a tray would.
* **Gaps while Explorer's window is first.** Programs address whichever window `FindWindow` returns. If Explorer's `Shell_TrayWnd`
  comes in front of ours (it was remade, or it re-asserted its place), messages sent in that gap go to Explorer alone and the model
  does not hear them; the keeper raises ours again within 250 ms, and nothing reconciles the model afterwards. An icon that a program
  deleted while it stayed alive (hiding its icon) in such a gap stays in the model as a ghost until the program adds again or ends
  (the sweep drops icons whose owner window is gone, not ones whose owner merely forgot them). A program that adds an icon in the
  gap is missing from the model until it adds again. Replaying `TaskbarCreated` to repair this was considered and left out: programs
  that ignore it would then be lost from the model, which is worse.
* **Two copies**: the dock is a single instance per session, so there is one host. A second tray host, of another program, is not a
  forwarding target (unless asked), and a second copy of rustyfiles.exe is not either (loop guard).

## LOG-ONLY mode

`RUSTYFILES_TRAY_LOG_ONLY=1` (exactly "1"). The window exists, everything is passed on and the replies are Explorer's, but: no
`HICON` is read, no tip, balloon text, title or program name is kept, the model stays empty, rectangle queries are not answered,
nothing is clickable. The host only counts, and remembers the KEYS of the icons it saw so it can say how many there are. The dock shows
nothing in any mode. The log gets, at most every 30 s and only after a change, one line of counts:
`tray host: 12 icons seen; messages 40, notify 38, adds 12, modifies 20, forwarded 40`.

## Steps of a first careful try (for a human)

Nothing here needs an administrator, writes the registry or touches Explorer's files or settings. **What is and is not undone by
stopping the process:** Explorer's own tray list is never changed by us except through messages that programs themselves sent,
passed on untouched, so it is complete when we stop. Ending the process removes our window and Explorer's is first again. A
program that FOUND our window and kept its handle (only if shell32 caches the lookup, which is not measured) fails its tray calls
until it looks again, is restarted, or Explorer restarts; a normal stop (the setting off, quitting the dock, `Drop`) destroys the
window first and then broadcasts `TaskbarCreated`, which sends such programs back to Explorer; a kill does not broadcast. So a kill
is not a perfect undo, and a clean stop is.

1. **Baseline, read-only.** `cargo run -p rf-win --example trayhost_probe` (or the built exe) and keep the output: exactly one
   `Shell_TrayWnd` window, owned by `explorer.exe`; its rectangle; its property names (`TaskbandHWND` among them). Count the icons in
   Explorer's tray by eye (and the hidden ones in the overflow) and note a program with taskbar progress (a copy in rustyfiles shows it).
2. **A test copy with the host in LOG-ONLY mode and NO start broadcast.** Config folder `%TEMP%\rf-tray-try` with a `settings.toml` that contains
   `shell_tray_host = true`. If a rustyfiles dock of yours is running, a second `--shell` greets it and ends (single instance per
   session): close the real dock first and start it again afterwards. Run:
   `set RUSTYFILES_CONFIG_DIR=%TEMP%\rf-tray-try & set RUSTYFILES_MULTI_INSTANCE=1 & set RUSTYFILES_MUTE=1 & set RUSTYFILES_TRAY_LOG_ONLY=1 & set RUSTYFILES_TRAY_NO_START_BROADCAST=1 & rustyfiles.exe --shell`.
   The log must say `tray host: starting (log only: yes, ..., start broadcast: no)`, then `tray host: window made (log-only: ...); Explorer's tray window found`.
   The start broadcast is left out on purpose for this first run: what it does to Explorer (programs adding icons Explorer already has,
   probe P7) is not measured, and it is the one thing the start does that reaches other programs. The stop broadcast cannot be left
   out and is sent at the end (it is what puts programs back with Explorer).
3. **Look.** Run the probe again: now TWO `Shell_TrayWnd` windows, ours FIRST, hidden (`visible: false`), with Explorer's rectangle, its
   `TrayNotifyWnd` child, and the same property names. Explorer's tray must look exactly as in step 1: same icons, same hover tips, a
   click on an icon still works, taskbar progress still shows. Without the start broadcast the host only counts what programs send
   from now on, so the icon count starts at 0 and grows as tooltips change.
4. **Make traffic.** Start a program with a tray icon, change a tooltip (volume, a download), quit it. Wait for the 30 s line: the
   counts rise (`adds`, `modifies`, `deletes`), `forwarded` equals `messages` minus `no_target`, and `unparsed`, `unknown_kind`,
   `handler_panics`, `forward_timed_out`, `forward_target_gone` stay 0 or small. **Any `unparsed` count is the finding the plan is
   waiting for**: the layout differs from the document; keep the build and send the counts. Also read `explorer_refused` (icon
   requests Explorer answered with 0), `raised` (the keeper had to put ours back in front; a number that keeps growing means
   Explorer fights for the place), `local_registered` and `local_sent_by_others` (messages to the first `Shell_TrayWnd` that are not
   passed on; see the forwarding rule).
5. **Stop it** (quit the test copy from the dock's menu, or end it in Task Manager): run the probe: one `Shell_TrayWnd` again, Explorer's.
   Explorer's tray must be exactly as before, with every icon. The stop line in the log says `tray host: stopped; ...`.
6. **Without log-only** (the same command without `RUSTYFILES_TRAY_LOG_ONLY`; still without the start broadcast): same as 3 to 5; the
   log now says `icons held`. The model fills (visible only in the log counts; the dock draws nothing yet). Only after this went
   well, once **with** the start broadcast (leave `RUSTYFILES_TRAY_NO_START_BROADCAST` out): programs now add again, and
   `explorer_refused` and `re_adds` show what Explorer did with the duplicates; Explorer's tray must still be as in step 1.
7. **Rectangles (probe P6), only after 6 went well:** add `RUSTYFILES_TRAY_RECT_PACKING=corners`. The dock must report rectangles for
   that, which needs the strip (D3); until then the host has no rectangles and Explorer answers everything. This step belongs to D3.
8. **Start and stop 20 times** with a few tray programs open; Explorer's tray must keep every icon after the last stop.
9. Anything odd (an icon missing in Explorer, a program whose tray icon does not react, progress on a taskbar button gone): stop the
   copy at once (quit it from the dock's menu or the setting off, so that the stop broadcast is sent; end the process only if the
   dock does not answer), run the probe, and write down which program, and whether it runs elevated or at a LOW integrity level
   (see the integrity note above). `RUSTYFILES_TRAY_VISIBLE=1` is the first thing to try for a program that seems not to find the
   window. If the log says the host thread has not ticked, end the dock process: nothing else can remove the window.

## What was NOT run (the real paths)

Run by tests: the model, the pixel arithmetic, the handler with fakes (including re-entry and a fuzz over random blocks), the relay
with fakes (cache, restart, retry), the click tables with fakes, the lifecycle order with fakes, the strip order, the rf-app settings
glue, the source scans (nothing but `win.rs` touches Windows; no test creates a window, sends or posts a message, or starts the
host), and GDI picture reads from icons the tests make themselves (colour with alpha, monochrome, a 128 x 128 icon, a bad handle).

Also run by tests (all windowless): the order of the pass-on against the pixel reading, Explorer's verdict on adds / modifies /
deletes, the start handshake under two racing threads, the stall watch, the rule for the properties Explorer withdrew, the settings
reload decision (`decide`), the log-line scan (including field accesses and inline format arguments), and source scans that the
250 ms tick does not enumerate windows and that `WM_CLOSE` goes through the window object.

NOT run: `TrayHost::start`, the window class and window, the window procedure, `SendMessageTimeoutW` to Explorer, the property
copying, `EnumWindows` for the class, `SetWindowPos(HWND_TOPMOST)`, the broadcast, `SendNotifyMessage` to an owner,
`AllowSetForegroundWindow`, reading a real program's `HICON`, the host with a real settings reload, and the whole behaviour of
Explorer and of real programs against all of that. The probe reads windows, properties and rectangles (no message) and runs the
handler over fakes.

## Deviations from the plan, and why

* **Forward limit 4 s, not 1.5 s** (plan 5.3 says 1500 ms): shell32 waits 4 s for the taskbar; a shorter limit fails calls that would
  have succeeded, and Explorer would still process the message afterwards, so a program that tries again adds twice. `SMTO_ABORTIFHUNG`
  keeps a hung Explorer from costing the 4 s. The same figure as the sender's own wait is a known weakness: our wait starts after the
  sender's, and while this thread waits for Explorer, other senders' messages are delivered to it and must finish first (last in,
  first out), so with a SLOW Explorer (seconds per message, for example when the start broadcast makes dozens of programs add at
  once) an outer sender can give up before its reply is ready although Explorer processed its message. That needs an Explorer that is
  already slow; it is not measured, and a lower limit would fail the calls that would have succeeded. `forward_timed_out` counts it.
* **Rectangle replies are off until the packing is named** (plan D2: "answers Shell_NotifyIconGetRect from the rectangle the
  interface reports"). The code does it, for both packings, tested; the default is Explorer's answer because the packing is unverified.
* **Dead owners: a check at every click plus a sweep every 5 s on the existing tick**, not `RegisterWaitForSingleObject` per process
  (plan 4.4). Rare enough, cheap (at most 256 `IsWindow` calls), and no extra handles or callbacks. A wait per process is the
  upgrade if the sweep ever shows up.
* **Ids are a counter, not slab indices** (note 5.4): an id is never reused.
* **Pictures are straight-alpha BGRA, not premultiplied** (note 5.4 says premultiplied): the same convention as `rf_win::dock::icons`,
  which the interface already draws. Premultiply when uploading if the renderer wants it.
* **Explorer's `Shell_TrayWnd` is never parked at `HWND_BOTTOM`** (ManagedShell does): only our window is moved, so Explorer's is
  never changed. If the 250 ms keeper turns out not to win, that is a finding for the first try.
* **Eight properties are copied** (not in the plan's list; an allow-list, see above): so that programs reading `TaskbandHWND` from the
  first `Shell_TrayWnd` still find Explorer's taskbar button window. Without it ITaskbarList3 progress and overlays could silently
  stop working while the host runs. Whether anything reads the others from the first window is not known.
* **Dead owners and the owner's identity.** The process recorded for an icon is the process of the window NAMED in the block, not of
  the sender (Explorer behaves the same: any program can add an icon for another program's window; a click then goes to that
  window). The sender of a `WM_COPYDATA` is not compared with it (the meaning of `wParam` there is from documents, not measured).
  An icon whose window was not a window when it was added (process unknown) is never sent to and is dropped by the next sweep. The
  check happens once per click, before the up to three messages of the sequence; a handle reused in the microseconds in between is
  not closable (the messages are queued to the owner by `SendNotifyMessage` anyway).
* **The strip does not exist yet**, so the click engine and the rectangle book have no caller in the app; they are exercised through
  fakes only.
* **No balloon queue or drawing** and no stripping of `NIF_INFO` from the forwarded copy: the model keeps the latest balloon of each
  icon and announces it; the notification increment (D4) does the rest. Messages are forwarded untouched, as the task says.

## The probe

`cargo run -p rf-win --example trayhost_probe` prints JSON: the `Shell_TrayWnd` windows now (handle, program, rectangle, extended style,
visibility, the `TrayNotifyWnd` child's rectangle, property NAMES), the `TaskbarCreated` message id, and a run of the handler over
made-up blocks with a made-up Explorer, with and without an Explorer and with each rectangle packing. It creates no window, sends and
posts no message, broadcasts nothing and starts no host.
