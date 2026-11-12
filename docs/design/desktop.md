# The desktop (experimental)

The desktop that rustyfiles draws itself: the wallpaper and the icons of the user's Desktop folder, one window per monitor, over
Explorer's desktop (`shell_desktop_cover`, off by default; `docs/design/shell-host.md` has the windows, the z-order and the
scratch-folder rules). This file says what the desktop does **now**, item by item, and what it still does not do. Source:
`crates/rf-app/src/shell/desktop/` (the view), `crates/rf-core/src/desktop/` (the pure rules), `crates/rf-win/src/desktop/` (Win32).

Nothing changes for a user who has the switch off: no window, no watch, no thread, no read.

## Settings: Masaüstü (deneysel)

A page of its own between "Dock" and "Sayfa düzeni", marked with a small "Deneysel" pill in the page list and "(deneysel)" in its title
(the Dock page is marked the same way). Built like the Dock page: one list of entries (`desktop_page.rs`) that the page draws and the
Settings search reads.

| Setting | Field | Default |
|---|---|---|
| Masaüstünü rustyfiles çizsin (deneysel) | `shell_desktop_cover` | off |
| Masaüstü simgelerini göster: follow Windows / show / hide | `desktop_show_icons` | follow |
| Bu bilgisayar, Kullanıcı dosyaları, Ağ, Geri Dönüşüm Kutusu, Denetim Masası: follow Windows / show / hide each | `desktop_icon_this_pc`, `_user_files`, `_network`, `_recycle_bin`, `_control_panel` | follow |

* **Follow Windows** (the default) is exactly what the desktop did before: `HideDesktopIcons\NewStartPanel` and `Explorer\Advanced\HideIcons`
  decide. **Show** and **Hide** are this app's own choice, applied in memory on top of what Windows says
  (`rf_core::desktop::virtuals::apply`). **Windows' settings are never written**, so Explorer's desktop and ours may disagree after a
  choice; going back to "follow" brings them together again. (Open decision for the user: also writing Windows' values would keep the two
  in agreement, and would break the rule that nothing here writes the registry. Not done.)
* An organisation's `NoDesktop` policy still wins over every choice (tested).
* Where Windows has "show desktop icons" **off** (`HideIcons = 1`, as it has on the development PC, which also has This PC on), the cover
  desktop with every choice on "follow" shows no icon at all, files included, because Explorer's would not either. Setting
  "Masaüstü simgelerini göster" to "Göster" brings them back; it is the one switch to flip first on such a PC.
* A scratch folder shows no system icon unless the choice says "show" for it (then the Recycle Bin on it is the real one).
* Moved off other pages: only `shell_desktop_cover` (its row, with the same title, text and rules: it needs the dock to be switched on,
  and can always be switched off) left the Dock page, where it sat under "MASAÜSTÜ VE KABUK". That section is now "GÖREV ÇUBUĞU" and holds
  `shell_hide_taskbar` only. The developer page ("Geliştirici araçları") and the advanced page ("Gelişmiş") already held nothing of the dock,
  the desktop or the shell host (the Dock page commit took the block away); a test keeps it so.
* NOT on the page, on purpose: icon size, align to grid, auto arrange and the sort. They are the desktop's layout (`desktop-layout.json`),
  written by the process that draws the desktop (the dock's process, which never saves the settings); the page lives in the main app and a
  second writer of that file would lose what the first wrote. They are on the desktop's right-click menu and the page says so.

## The system icons

This PC, the user's folder, Network, the Recycle Bin, Control Panel. Complete icons:

| | |
|---|---|
| Picture | the shell's own, asked for the icon's state: the Recycle Bin has one for full and one for empty and is asked again when it flips (the old picture stays up until the new one has come; a plain glyph stands in before the first). The user's folder and the others are one picture each. |
| Notices a change | a directory watch on the user's bin folder (`X:\$Recycle.Bin\<SID>`) of every fixed drive that has one (`rf_win::desktop::binwatch`), debounced 300 ms; it works for a delete by any program, for a restore and for emptying. The watch exists only while the bin's icon is shown (none for a hidden icon). A bin folder that appears later (a drive's first delete creates it) is picked up at the next read, which every file operation of the app causes when it ends, and after our own Empty. A question the shell did not answer in time keeps the picture the bin had (it is never read as "empty"). No timer. |
| Name | the shell's localised name (loaded also for a scratch desktop now) |
| Opens (double click, Enter) | This PC and the Recycle Bin in our own windows (the Recycle Bin view included); the user's folder as the folder it is; Network in our window when the shell lets us list it; Control Panel is Explorer's own (an applet list: no file view of ours). In the dock's process the folder is handed to the main rustyfiles as before. |
| Menu | Recycle Bin: Open, Empty Recycle Bin (Windows asks first; dimmed when empty, in the protected preview and on a scratch desktop, whose bin icon is the user's real bin; Windows' own menu behind "more options" is not offered for it there either), Properties (Windows' Recycle Bin properties); This PC: Open, Properties (Windows' System page); user's folder: Open, Properties (our window, locked); Network: Open, Properties; Control Panel: Open. All with "Show more options" (Windows' own menu). Cut, Rename, Delete are dimmed. |
| Layout | placed, moved, selected like any icon; they can now also be moved to another monitor (below) |
| Drop | files dropped on the Recycle Bin are deleted to the bin through the app's delete (confirmation per the setting, Status Center, undo); the Public Desktop's files are refused as before |
| Protected | cannot be renamed, deleted, cut or have anything dropped into them (This PC, Network ...): a notice says why |

## What else the desktop does

* **Items**: the user's Desktop and the Public Desktop merged, hidden/system rules, watched; shortcuts show no extension and now carry a small
  arrow badge on the picture (our own drawing; not Windows' overlay).
* **Selection**: click, Ctrl, Shift, rubber band, arrows (column-major neighbours), Shift+arrows, Home/End, type-to-select, Ctrl+A.
* **Operations** (the app's own engines, with undo and the Recycle Bin): open, open many (asks above 15), copy/cut/paste, delete,
  Shift+Delete, new folder/text file, in-place rename (Explorer's rules), drag icons onto a folder/program/the bin, drag files out and in.
* **Item menu**: Open, Open with… (one file), Create shortcut (on the user's own Desktop, also for a Public Desktop item), Copy path, Properties
  (Specifics for system icons above), Windows' own menu behind "Show more options". What is in it for a given selection is
  `menu_plan::item_menu` (a tested table).
* **Menu of the empty desktop** (`menu_plan::background_menu`): Icon size, Align to grid, **Auto arrange icons**, **Show desktop icons**, Sort,
  Refresh, New (+ Windows' templates), **Paste shortcut**, **Undo / Redo with what they would undo** ("Geri al: Silme"), Open in rustyfiles,
  **Open in terminal**, **Display settings**, **Personalize** (Windows' Settings pages), Windows' own menu behind "Show more options".
* **Auto arrange** persists (it used to apply for one sort only): icons are laid out in the sort order and stay; one put down elsewhere does
  not land (a notice says why); sorting keeps it on; turning it off freezes the arrangement on the screen into the stored positions.
  **Show desktop icons** from the menu is kept in the layout file; the settings' "Gizle" cannot be undone from the menu (the item is dimmed).
* **Where new, pasted and dropped items land**: next to the pointer when the user asked with the mouse (right-click New / Paste shortcut /
  a drop on an empty place, Create shortcut stays at the first free cell): `rf_core::desktop::arrival::place_near` (tested with auto
  arrange on and off, a taken cell, free offsets, several arrivals). Ctrl+V, Ctrl+Shift+N and files another program made go to the first
  free cell as before, whatever was right-clicked earlier: the point belongs to one use of the menu (`menupoint`, tested), a key never takes it, and a press on an icon or a menu opened from the keyboard spends it.
* **Between monitors**: system icons and Public Desktop items (which cannot leave as files) are dropped on another monitor by the layout alone
  (`arrival::move_across`, tested); files of the user's own keep the system's drag as before, which already worked between the monitors'
  windows. The icons are not drawn while they cross (the other monitor's window shows them once the button is up).
* **Cost**: with the switch off, nothing. With it on: one more directory watch per fixed drive with a bin (idle), one state read of the bin
  (a shell call off the UI thread) per burst of changes, a handful of comparisons per tile per frame (the badge); the file list is untouched.

## Still missing

Must-have gaps a reader listed after comparing with Windows 11's desktop, and what became of each:

| Gap | State |
|---|---|
| Choice of the system icons | **done**, as an override only (Windows' values not written) |
| A Masaüstü settings page | **partly**: the page, the cover switch, show icons and the five choices are done; icon size, align to grid, auto arrange and the sort are **not** on the page (they are on the right-click menu; the page cannot write the layout file the host owns, and a second source of truth would drift). Needs a command channel from the main app to the host |
| Auto arrange, Show desktop icons in the menu | **done** |
| Recycle Bin picture and notice | **done** |
| Menu extras | **done** (Undo/Redo, Paste shortcut, Display settings, Personalize, Terminal) |
| New/pasted/dropped items at the pointer | **done** |
| Item menu gaps | **done** (Properties of system icons, Empty Recycle Bin, Create shortcut, Open with) |
| Every kind of icon between monitors | **done** for system icons and the Public Desktop's; no ghost on the target monitor |
| Shortcut arrows and the elevation shield | arrow **done**; the **shield is not** (it needs the program's manifest read off the UI thread; left out rather than half done) |
| Dragging with the right button; copy/move/link by modifier keys | **not done**: the right button still opens the menu, and Alt / Ctrl+Shift do not make shortcuts. Needs a drop menu (Copy here / Move here / Create shortcuts here / Cancel) and the link effect in the pointer hint |

The brief that produced this file carried only the must-have list: there was no "should" or "later" list to carry over.

Also not the same as Explorer's yet (from `shell-host.md`): plain folder pictures where Explorer previews the content; an icon whose Explorer row
is under our dock moves to a free cell above it; only the first virtual desktop shows our icons.

## Not run live

See the report of the increment: the real paths of Properties (shell verbs on `::{CLSID}`), Empty Recycle Bin, the bin watch, Open with,
Create shortcut and the cross-monitor drop have only been built and unit-tested over fakes or read-only probes.
