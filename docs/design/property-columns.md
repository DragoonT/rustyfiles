# Columns for any Windows property

Explorer lets the user add any property of the Windows property system to the Details view (dimensions, duration, artist, album,
date taken, authors, bit rate, camera model ...) and sort and group by it. Music and photo folders need that. rustyfiles had
fixed columns only (name, tags, modified, created, type, size, git, sync). This adds the others, off until the user asks.

## What the user sees

* **Right click on the Details header** opens a menu (there was none): the four fixed date/type/size columns as check items, the
  property columns chosen so far (a click removes one), and **"Diğer sütunlar…" / "More columns…"**.
* **The chooser** is our own dialog painted over the window (`prop_chooser.rs`): a search box and a virtual list with check marks.
  Without a query: the common properties first, under a heading per kind of file (pictures: dimensions, date taken, camera model
  and maker, bit depth; music: artist, album, title, track number, duration, bit rate, genre, year; video: duration, frame width,
  frame height, frame rate, total bit rate; documents: authors, title, page count, word count), then **every other property the
  system lists** as "viewable" (356 on the machine where it was written), by display name in the UI language (that is the
  system's own localized name). With a query: the properties whose display name or canonical name contains it, common ones first.
  A click on a row adds or removes the column at once; at most 8 property columns.
* **The columns** come after the fixed ones. The header cell has the property's display name, a resize handle on its left edge
  (dragged like the other columns, the width is kept in `settings.toml`), and sorts on a click (a second click flips).
  Numbers are right aligned. A cell stays empty until its value has arrived.
* **Sorting** by a property: numbers as numbers, dates as dates, text with the app's natural order, files without a value last in
  both directions, folders first/last as the folder's settings say. The list keeps its order while every file's value is read; the
  status bar shows "Özellikler okunuyor… 1234 / 5000"; the order is applied once, when all values are in. Sorting again, sorting
  by a fixed column, or going to another folder cancels the read.
* **Grouping** by a property: "Gruplandır" (sort bar menu and the folder's background menu) gets one entry per chosen property
  column. Same background read, groups made once. Dates group by month (newest first), text and numbers by value; a property
  with many distinct values is cut into ten groups of equal size (numbers, labelled "a – b") or grouped by first letter (text, over
  200 distinct values). Files without a value form the last group, "Belirtilmemiş".
* Where the pane is too narrow the property columns are placed first and the fixed columns share what is left (the name keeps
  180 px, as before); a property column that does not fit is not shown.

Not remembered: the sort and the grouping by a property are not stored with the folder's layout (the layout stores a sort key
and a group index; a property is neither). The chosen columns are global, not per folder type as in Explorer.

## Where it lives

| Piece | File |
| --- | --- |
| Chosen list and its settings form, typed values and their order, sorting and grouping by values, the cache and its key, the visible-range request model, the chooser's row order | `crates/rf-core/src/propcols.rs` (tested) |
| The property list (canonical + display names), the reader behind a trait (`PropSource`: `SystemSource`, `FakeSource`), the batch runner, the row queue | `crates/rf-win/src/propcols.rs` (tested) |
| The view side: header cells, row cells, requests, jobs, menus | `crates/rf-app/src/folder_view/prop_cols.rs` (a child module of the folder view, so it reaches the view's private state) |
| The chooser | `crates/rf-app/src/prop_chooser.rs` |
| Probe | `crates/rf-win/examples/propcols_probe.rs` |

Hooks in files that existed before (all small): `folder_view.rs` (module line; one field; `GroupBy::Property`; width left for the
columns in `visible_cols` and `spare_width`; header cells, a right-click and a drag handler in `render_header`; the arrow of a fixed
column; cells and the request in `render_rows`; `set_sort`, `set_sort_spec`, `set_group`; reset in `load_inner`; carry the groups by name and re-sort in `swap_in`),
`ctx_menu.rs` (two group menus), `workspace.rs` (status line, overlay, one overlay flag), `settings.rs` (`columns.properties`),
`lib.rs`, the i18n tables.

## How the values are read

**`IPropertyStore`, not `IShellFolder2::GetDetailsEx`.** One store per file gives every chosen property for the price of one open (a
handler is created once, not once per column). The values come typed (a `PROPVARIANT`), so a sort can order numbers and dates as
numbers and dates; `PSFormatForDisplayAlloc` turns the same variant into the text Explorer shows (units, localized dates, "a; b"
lists). `GetDetailsEx` needs the parent folder bound and a child ID list built per file, and hands back text for the column
provider's idea of the property, not the typed value.

**Flags.** `GPS_BESTEFFORT | GPS_NO_OPLOCK`, and not `GPS_OPENSLOWITEM`. Best effort: a handler that fails still gives the file
system's own properties. No oplock: reading does not take the file's opportunistic lock, so a program writing the file is not made
to wait. Not "open slow items": a slow item (a slow share) is not forced open. The probe compared the other flags on 200 files of a
mixed folder (counts of values, same files): `GPS_FASTPROPERTIESONLY` returned **0** values for all five common properties (media
properties are not "fast"), so it is no use; `GPS_PREFERQUERYPROPERTIES` returned the same values as the default and took the same
time once the system's caches were warm (419 ms against 407 ms; the cold first run was 1235 ms), so there was no reason to add it.

**The online-only rule.** A file with `FILE_ATTRIBUTE_OFFLINE`, `RECALL_ON_OPEN` or `RECALL_ON_DATA_ACCESS` (OneDrive and other
placeholders) is **never opened**: opening a property handler reads the file and a read downloads it. Such a file gets no value at
all. It is checked twice, from the listing's attributes (no request is made) and again by the reader itself with
`GetFileAttributes` through `symlink_metadata`, which does not recall a placeholder. Folders are not read either.

**Off the UI thread, one thread, one shell thread.** Every shell call runs on the process's one shell thread (`rf_win::sta`, the
doc comment there: a worker that called the shell itself would become an apartment that never pumps), one file per call so that a
context menu is not made to wait for a batch. Two limits, kept apart (`call_timed` in `rf_win::propcols`): a file may **wait 10 s
for its turn** on that thread (it may be busy with a menu, an icon or another file) and may **take 2 s once it has started**.
`sta::call` has one limit for both, which charged a file for the time other shell work spent in front of it. A file that did not
get its turn in time is skipped when the turn comes (nobody is waiting for it). A file whose handler is slow once started cannot be
abandoned: the job still runs to its end on the shell thread. Therefore **three files in a row that run out of time stop the
batch** ("stalled"): a queue pauses for 10 s, a sort or grouping job gives up and says so in the status bar.

A file that ran out of time is **not stored as "no value"**: its cell stays empty, it is not asked again for 60 s (so that every
answer does not start the same request over) and is tried again by the next scroll over it after that. Files the system refused or
could not find are stored as empty until their size or time changes.

**Which rows.** Only rows that are on screen plus 30 ahead and 30 behind (`rf_core::propcols::plan`: visible first, then ahead, then
behind, at most 150 at a time, those not in the cache yet). `render_rows` notes the first and last item it drew; the plan is made
only when that range, the entries or the cache changed. The plan **replaces** what has not started: rows that scrolled away are never
read; the file being read is finished and is not queued twice. A navigation clears the queue. The queue's thread (`rf-propread`)
starts with the first request and ends after a minute without work or when the tab is closed.

**The cache** (`PropCache`, shared by all tabs, in memory only): per path, the size and modified time the values were read with and
the cells read. A file whose size or time changed reads as never read. Bounded at 150 000 files (about 300 bytes a file and 100 for
each property); past that an eighth of the entries is dropped to make room, not all of them. A sort or grouping job does not depend
on the cache for its result: it keeps every value it read (and the ones it found in the cache at the start) in a list of its own,
so an entry the bounded cache drops while the job runs cannot turn into "no value" in the order.

**A sort or grouping** (`rf-propjob` thread, one per request): counts the files without a value in the cache, reads them with the same
reader (progress in an atomic counter that the status bar shows), then, off the UI thread, builds the order (`sort_order`) or the
group keys (`group_keys`) and sends them over. The view installs the order where it keeps its sort order (`sorted_cache`), so the
filter, grouping and soft reloads use it. If the entries changed while the values were read the job runs again (cached values make
that quick); the first news from the folder watcher starts it at once, news within the next second is one more start, not one each.
A grouping keeps each file's group by name (and the folds the user made) while the groups are made again. A new navigation,
another sort, another grouping or **closing the tab** cancels the job (`PropState` ends it when it is dropped).

The Sort menu follows the sort in force: when a property sort has been applied the menu's direction is the property's, so
"Artan"/"Azalan" sort the property in that direction, and the item shown as chosen ends the property sort (as the chosen fixed
key does). Groups are in the order of their values (dates newest first), whatever the sort direction, as the built-in groupings are;
a number property cut into ten groups is cut only where the value changes.

## What it costs

**Off (no property column chosen, the default): nothing.** No thread, no allocation per row, no COM call, no drive-type query (that
is asked only after the settings hold a column). What remains is a check of
one `Vec::is_empty` on the settings in: `visible_cols`/`spare_width` (`props_used_width`), `render_header`, `render_rows` (one call
before the loop, one after), plus two `Option` checks in `load_inner` and `swap_in`. The settings file has no new key
(`skip_serializing_if`). `GroupBy::Property` is not in `GroupBy::ALL`, so no layout or setting can hold it.

**On:** per frame, for each drawn row one `path_of` and one lock of the cache (40 or so rows), and a hash of the visible rows' indices (the
plan is made again when other files come to stand on the rows: a sort, a filter or a fold); no work when nothing moved (the plan key is
the same). While a sort or grouping by a property is in force and the column does not fit the pane, nothing is done per frame beyond
one comparison of the column's name with the chosen list. Reading: the probe read 200 mixed files from a Downloads folder in 1.2 s cold (6 ms a file) and about 0.4 s warm (2 ms a
file), counting only the five properties asked for; a media file whose handler parses a long container costs more, a plain file
less. A folder of 100 000 files sorted by a property is therefore minutes of background reading the first time (it can be cancelled
and the cache keeps what was read), the second time seconds. The UI thread never waits for any of it; the listing of the folder is
unchanged (the values are never part of it).

## What was not run

Everything below needs the real window and was not run (nothing may be shown on the user's screen while this was written); the
lead checks it live (see the report):

* the header, the cells, the arrows, the resize handle, the chooser and the menus **drawn**; no pixel of the new UI was seen;
* a sort or a grouping by a property on a real folder, and the status bar line while it reads;
* a folder of 100 000 files with a property sort (time and memory not measured; the cache arithmetic above is an estimate);
* online-only files: the rule is unit-tested on attribute values and the reader checks attributes first, but no real placeholder
  was read or skipped;
* a slow or broken property handler: the stall logic is tested with a fake source; no real stuck handler was met;
* a network share, a phone (MTP) and archives (property columns are off there by design: only plain folders of the file system;
  a UNC path or a mapped network drive is recognised by `rf_win::localized::is_network`, asked once per opened folder; the rule
  is unit-tested on paths, no real share was opened);
* a handler that really hangs: see the limit below.

## A limit that stays

The shell thread is one for the whole program (menus, dock icons, the desktop view, thumbnails and these reads). A property
handler that really hangs, once it has started, holds that thread for as long as it hangs, and nothing else that needs the shell
thread is answered meanwhile (a "more options" menu, an icon), because the job cannot be abandoned. What was done about it: the
reads stop after three files in a row that run out of time (nothing more is queued behind the stuck one), a file that did not
answer is not marked empty for good, and a file only counts as late for the time it ran, not the time it waited. What was not
done: a thread of its own for property reads. `sta.rs` records that several apartments asking the shell at once broke icon
extraction, and that cannot be tried without the real window, so the reads stay on the shared thread. A handler that crashes the
process takes the program with it; nothing isolates it.
