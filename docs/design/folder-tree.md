# The folder tree in the navigation pane

Explorer's navigation pane is a tree that opens and closes; rustyfiles' sidebar is a flat list (pins, drives, tags). This adds the
tree as one more section of the sidebar, **off by default**. Nothing changes for a user who does not turn it on.

Code: the model is `crates/rf-core/src/navtree.rs` (pure, tested without a window); the view and the window's hooks are
`crates/rf-app/src/sidebar_tree.rs` (a child module of `workspace`, so that `workspace.rs` only gets small hooks).

## What it does

* Settings: `sidebar_tree` (default off) and `sidebar_tree_follow` (default on). Both are switches in Settings, under "Kenar çubuğu
  bölümleri" ("Klasör ağacı", "Açık klasörü izle"). The tree can also be switched from the window's tab-actions menu (the one that
  has "Kenar çubuğu"). While the tree is on, a right click on the sidebar's own ground (the section headers, gaps and rows
  that have no menu of their own; rows that have one keep it) offers the two switches. With the tree off the sidebar has no
  such handler: a right click on those places does what it did before (nothing).
* With the setting on, a "Klasörler" section sits right under the pinned folders, above the drives and the tags (moved there at the user's request). After a navigation the sidebar is scrolled to the open folder only when the pointer is not over the sidebar: a click on a pinned folder must not scroll the row away from under the pointer. Its roots are the user's profile
  folder first, then the drives (the same list the sidebar's "Sürücüler" section uses, so a USB stick appears and goes with it).
* Every folder row has a chevron (12 px, the size the rest of the app uses), a 16 px icon like the sidebar's rows, and the name
  (the localized name when Explorer has one: "İndirilenler"). Indent is 16 px a level, up to 6 levels; deeper rows stand at that
  indent (the name would have no room in a narrow sidebar) and show the whole name as a tooltip. The selected row is drawn as the sidebar
  draws its selected row (selection fill and the accent pill).
* Children of a folder are its folders only (files are not tree nodes), hidden and system ones by the same three settings the file
  list uses (`show_hidden`, `show_system_files`, `show_dot_files`), sorted the way the file list sorts names (natural order,
  case-insensitive, on the localized name). Changing those settings re-filters at once, without listing again.
* A folder is listed **when it is first opened**, on a worker thread (`rf_win::list_dir`, the file list's own lister; no shell
  call, so nothing goes through `rf_win::sta`). While the first listing takes longer than 300 ms the chevron becomes a quietly
  pulsing spinner; it pulses three times (2.7 s) and then stands still, so that a listing that hangs (an offline share) does not keep
  the window drawing. A listing that fails (no access, drive not ready…) leaves a quiet one-line row under the node; a click on the
  row tries again. A folder that turns out to be gone leaves the tree.
* Click: navigates the active pane (the focus stays in the tree, as in Explorer; the folder list is not refocused). Click on the
  chevron: opens or closes only. Middle click and Ctrl+click: open the folder in a new tab (in the background, like the sidebar's
  other rows do with the middle button). Right click: the sidebar's folder menu (`sidebar_menu`: open, new tab, new window, pin,
  copy path, terminal, our Properties, and Windows' menu behind "more options"). Dropping files or folders on a row copies or
  moves them into that folder with the same function the sidebar rows use (`drop_files`).
* Keyboard, when the tree has the focus (it gets it by a click on a row): Up / Down move the cursor, Home / End jump, Right opens
  a closed node or goes to its first child, Left closes an open node or goes to the parent, Enter navigates, Escape and Tab give
  the focus back to the file list, the menu key and Shift+F10 open the same Fluent folder menu, under the cursor row (not at the
  mouse, and never Windows' classic menu: only a Shift held at a *mouse* right click asks for that, as on the sidebar's rows).
* The rows are virtualised: the tree is a box as tall as all its rows inside the sidebar's scroll area, and only the rows in
  view (plus six above and below) are built, from the scroll position. The model keeps a flat list of rows (node and depth), so a
  tree with thousands of visible rows costs a few dozen elements per frame.
* The rail (the sidebar as a strip of icons) shows no tree, and neither does the full sidebar that opens over the content while the
  pointer is on the rail.

## "Açık klasörü izle" (follow the open folder)

After every navigation (the model is told the open folder once per change, not once per frame) the tree marks the open folder
selected, opens the branch down to it, and scrolls the row into view. The rule, which is `NavTree::follow`:

1. Only closed nodes **on the way** to the open folder are opened, one listing at a time (each answer continues the walk); nothing
   is ever closed by it. Sibling branches stay as they were.
2. A node the user closed by hand **less than 30 seconds ago** (a chevron click or the Left key) is not opened again, and the walk
   stops there: the closed node then carries a quiet mark ("the open folder is below me") and the view scrolls to it. After 30 s,
   or after the user opens that node by hand, the next navigation opens it again. So a user who tidies a branch away while
   browsing inside it is not fought, and a user who comes back to it later finds it open.
3. A folder that is not in the listing (hidden by the settings, or not a folder) ends the walk at its parent, the same way.
4. A navigation made by a click in the tree selects that very row and walks nowhere (the profile folder is a root **and** lies under
   its drive; without this a click on the drive's copy would jump to the other copy).
5. The scroll happens once, after the walk is over, and only if the row is not comfortably in view already.
6. With the follow setting off nothing is opened, listed or scrolled; the open folder is marked only when its row is already
   there.
7. Places that are not folders (Home, Settings, tags…) and folders outside every root (network paths, FTP) select nothing.

## Expanded nodes follow the disk

Every node that is open **and whose parents are all open** has a directory watcher (`rf_win::watch::watch_dir_changes`, the one
the file list uses; it is created on a worker thread because opening a directory handle on an offline share can block). When a
burst of changes arrives, the names are read off the UI thread (`stat_entry`, on the watcher's own thread) and the model applies
them: a folder made appears, a folder removed goes (with everything below it, and its watchers), a rename is a removal plus an
appearance, a case-only rename updates the name in place. If the watcher lost track, or a name cannot be read, the node is listed
again. A node is listed by one request at a time: reports that arrive while it is being listed ask for ONE more listing after
it, not one each, and a change that arrives during a listing makes that listing be followed by another one, so that an answer
that was started before the change cannot take the new folder away again. Closing a node (or one of its parents) drops its watchers. A node that was closed is listed again, quietly (no spinner),
when it is opened, with the open folders below it, because nothing watched them in between.

Limits, stated honestly:

* At most 32 folders are watched at once (the open folder's branch first, then the most recently opened). A tree with more open
  nodes than that has unwatched ones until they are closed and opened again.
* A rename loses what was open below the renamed folder (the new name starts closed). The model has the names only, not the
  rename pairs.
* A change that happens in the few tens of milliseconds between a node's listing and its watcher starting is not seen until the
  next change in that folder.

## State and persistence

The expanded set lives **in memory only**: it is kept for the session (closing and reopening the sidebar section or the rail does
not lose it; the setting going off drops the whole tree) and is lost when the window closes. Nothing is written to disk. The tree
of each window is its own.

## What it costs

* **Off (the default):** one boolean read per frame in the window (`ensure_side_tree`) and one extra item in the tab-actions
  menu. No entity, no thread, no watcher, no listing, no handler on the sidebar. The file list is not touched at all.
* **On:** nothing is listed until a node is opened. Each open node costs one listing (the file list's lister, off the UI thread)
  and one watcher (a notify watcher and a small thread each, so a typical tree of 3 to 8 open nodes is 6 to 16 idle threads; the
  cap of 32 watchers is at most about 64). Each folder row in the model is a few hundred bytes plus its strings; a folder that has
  100 000 subfolders open in the tree costs on the order of tens of megabytes and a rebuild of the row list on each change (a
  unit test builds 20 000 rows in well under a second in a debug build). A frame builds the rows in view only, and finding the open folder's
  row and the cursor's row is a table lookup, not a scan of the rows. Files are never nodes, but a listing reads every entry of
  the folder (`rf_win::list_dir`, the file list's lister) and keeps the folders: a folder with 100 000 *files* is enumerated in
  full, off the UI thread, each time it is opened, followed into or listed again, and (for a folder open in the tree that gets
  bursts of more than 256 names, such as an archive being extracted into it) again after each burst, one listing at a time and
  not more often than the watcher's debounce (250 ms and up). That is real disk work on a background thread, not nothing. The
  file list's own speed does not depend on the tree.
* **Memory is kept, within a budget.** Closing a node keeps its children (so that opening it again shows them at once while a
  quiet listing refreshes them). Over 30 000 nodes, closing a branch frees the children of closed branches, the ones opened
  longest ago first, down to 22 500; open nodes and the open folder's branch are never freed, so a folder with 100 000 subfolders
  stays in memory for as long as it is open. A freed branch is listed again, with the spinner, when it is opened.
* A listing has no cancellation: a listing of an unreachable share holds one of the background executor's threads (which the
  file list shares) until Windows gives up. (`list_dir` can be told to stop between batches; the tree does not use that.)
* The cost numbers above are estimates from the code and from unit tests, not measurements of a running window.

## What was run, and what was not

Run (no window was ever opened): the model's unit tests (32: keys, sorting, filter, roots and a root's new label, the follow rule
and its 30-second guard, the walk against a made-up disk and through a link that leads back up, changes made / renamed / removed,
a case-only rename with folders open below, a change during a listing on its way, one listing at a time, the freeing of closed
branches, the watch set and its cap, stale answers, the row lookup, the keyboard, and the arithmetic of the visible rows and of
the scroll goal);
tests of the view's disk side on a real temporary folder (the folder lister, the watcher's names, and a node that follows a real
folder made, renamed and removed while a real watcher runs); the whole `rf-app` and `rf-core` unit test sets; build and clippy.

Not run: **everything that needs the window**. The drawing (indent, chevron, spinner, selected row, the mark on a closed
ancestor), clicking, the middle and Ctrl click, the right-click menu and its "more options", drops on a row, the keyboard, the
scroll-to-row, the virtualisation against a real scroll position, the Settings switches and the two menus, the keyboard menu's
place under the cursor row, the tooltip of deep rows, the spinner stopping, the rail, and the look over the glass backdrop. Known
look difference: the selected tree row uses the Fluent fill and pill, not the glass lens the other sidebar rows use over glass;
when the open folder is also a pinned folder, the pinned row has the lens and the tree row the flat fill. The lens belongs to
one row per sidebar (the glass engine follows one probe), and the tree is drawn after the sidebar rows are chosen, so it was not
changed without a window to look at. The real
drive list and the real profile folder were not listed by a test. Windows' own menu from a tree row goes through the same
function as the sidebar rows' and has not been run from here either.

Not done: names are compared the way Windows compares them for this purpose (checked on this machine with
`CompareStringOrdinal`: "ı" and "I" are different, as in the model's lower-casing), but a folder whose name has a character
whose lower case differs from Windows' table is not looked for in other spellings; the tree does not take the focus from the keyboard (no shortcut moves the focus into it; click a row first); no
auto-expand while dragging over a node; no drag of tree rows themselves; no Explorer-style "show all folders" / "expand to open
folder" menu items beyond the follow switch.
