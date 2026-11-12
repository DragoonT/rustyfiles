# Jump lists (`rf_win::jumplists`, `jumplist_probe`)

What Windows' taskbar shows when you right-click an app's button: the app's Tasks ("New window"), the categories it wrote itself,
the items pinned to it, and its Recent or Frequent documents. The dock and the Start panel will show the same, so the shell has to
read them without Explorer's help. This is the data side only: **no interface, no removing of an item, no clearing, no pinning**.

```rust
let list = rf_win::jumplists::read(&AppKey::from_pin(&pin), 10);   // a worker thread; empty on any failure
rf_win::jumplists::open(&list.tasks[0])?;                          // starts the item through rf_win::dock::launch
```

`JumpList { tasks, custom: Vec<JumpCategory>, pinned, recent, frequent, which }`. A `JumpItem` is a title, an `Option<IconSource>` (the
dock's own icon type) and a `JumpTarget` (`File(path)`, `Link { program, arguments, working_dir, aumid }`, `Separator`). `AppKey`
is the identity as the dock knows it: the explicit AppUserModelID when the pin, the window or the package has one, and the program.
`AppKey::from_pin(&PinnedApp)`, `from_dock_app(&DockApp)`, `from_aumid`, `from_exe`. (The dock's key is the program path in lower
case; when the app's launch path is the same path, `from_dock_app` takes that one, with the program's own spelling, because the
hash is made of the real spelling and a lower-cased dotted capital I "İ" is not the letter it came from.)

`which` says what the app asked Windows to show in the automatic category, by what its custom file lists: `Recent`, `Frequent`,
`Both`, `Neither` (the app has a custom list of its own, with Tasks or categories, and names neither known category; Windows then
shows no automatic items for it, which is the documented rule for an app that writes its own list without
`AppendKnownCategory`; not checked against the taskbar's picture on this machine), or `Unknown` (no custom list at all, or an
empty one: Windows' default, Recent). `JumpList::automatic()` is what the taskbar would show in that category: Frequent for
`Frequent`, nothing for `Neither`, else Recent. `recent` and `frequent` still hold what the shell listed, whatever `which` is.

Off by default in the sense that nothing calls it yet: no setting, no switch, no window, no thread of its own.

## Where each part comes from

| part | source | how |
|---|---|---|
| `recent`, `frequent` | `IApplicationDocumentLists` (`windows.storage.dll`, in the process) | `SetAppID`, `GetList`, on the shell thread (`sta::call`), only for an app that already has an automatic file |
| `pinned` | the `DestList` stream of `<AppID>.automaticDestinations-ms`, and the stream of each pinned entry | pins in the pins' order; an entry with no stream stands for its `DestList` path |
| `tasks`, `custom`, `which` | `<AppID>.customDestinations-ms` | Tasks, the app's own named categories, and whether it listed Windows' Recent / Frequent category |
| the shell links inside | `IShellLink` loaded from a memory stream (`IPersistStream::Load`) | one call to the shell thread for all of them; never resolved |

When the shell does not give Recent (no answer in time, or an error), the file's own newest non-pinned entries stand in for it.
Frequent has no stand-in.

**The files.** `%APPDATA%\Microsoft\Windows\Recent\AutomaticDestinations` and `CustomDestinations`, read whole into memory with the
ordinary shared read (Rust's `File::open` shares read, write and delete, which is what Explorer allows). Explorer holds them open
and rewrites them; reading does not disturb it, and no copy is made on disk, so there is nothing to remove afterwards. A file
over 16 MB or a stream over 1 MB is not read. The automatic files are compound files, read with the `cfb` crate on the bytes in
memory (`Cursor<&[u8]>`; the crate's writer is used only by tests).

**Time.** `read` waits for the shell thread at most `BUDGET` (5 s) in all; what is left of the budget is each call's limit. A read
that runs out of time returns what it has (`Report::out_of_time` says so) and queues nothing more behind the call that timed out
(a test with a shell that hangs checks the time taken and the `within` of each call). On this machine a read takes 0.1 to 30 ms.
What is not bounded: reading the two files (local, shared read, a few kilobytes to a few megabytes) and the existence check of a
target on a fixed drive; and a job that timed out still runs to its end on the one shell thread (`sta::call` cannot cancel it).
That is why at most 200 links of the custom file (over all its categories together) and `2 * limit` of each category are sent to
the shell thread in one read. The known folders (a dozen shell calls) are asked once per process. There is no cache: the
caller decides how often to read (the files change when Explorer or an app writes them). A file that does not parse (a compound
file that does not open, a `DestList` with bytes that do not parse, a custom file not read to its end) is read once more after
15 ms, because Explorer rewrites these files in place and a read can land in the middle of a write.

**Dead targets** are dropped. A target is asked about only when it is on a fixed local drive (`GetDriveTypeW` says fixed or RAM
disk), and then only the file system's `metadata` is asked: not found is dead, access denied is alive. A drive letter that is not
a drive at all (`DRIVE_NO_ROOT_DIR`) is dead without asking anything; a path on a network share, a removable or optical drive, of
an unknown drive type, or anything not `X:\...` (relative, a `\\?\` prefix) is kept without being asked, so that a sleeping drive
or a dead server cannot hold the dock up. Entries with no file-system path (a library, a Control Panel page) are dropped from the
document lists. An icon location that is a package resource string (`@{...}`) is not an icon file, and the program's own icon is
used. Why entries are dropped, as the probe counts them on this machine: of the 14 pinned entries 2 have a target that is gone and
1 has nothing to open; of the 6579 other automatic entries 3706 have a target that is gone (recent documents that were deleted or
moved) and 559 nothing to open; of the 2086 entries of the custom files 1179 have a target that is gone (tasks of programs that
were uninstalled). None failed to load.

**Opening** (`open`) goes through the dock's launcher (`dock::launch::start`): a file or folder by the shell's "open", a task as its
program with its arguments, a packaged app's task by its AppUserModelID with its arguments (`dock::launch::start_packaged_with`,
the activation manager's argument string). The launcher starts a program in the program's own folder, so a task's working
directory is read and kept in the item but not passed on. `open` is behind the `Starter` trait; the tests
never start anything.

## The AppID: what names the files (settled on this machine)

The file name is the 64-bit CRC of the app's identity text, written in lower-case hexadecimal **without leading zeros** (4 of the 104
automatic names and 35 of the custom names here have 15 digits or fewer; this is the "names that are not 16 hex digits" of the
research note). The note had the polynomial, `0x92C64265D32139A4`, and not the rest. `appid::sweep` tried every usual parameter
set against this machine (both bit orders of the polynomial, init and xor-out of zeros or ones, input and output reflection on or
off, five text forms: upper-case UTF-16LE with and without a terminator, as written, lower case, upper-case UTF-8), counting how many
of the 273 apps of "All apps" named a file that exists:

| rank | polynomial | init | xor-out | reflect in / out | text | automatic files | custom files |
|---|---|---|---|---|---|---|---|
| 1 | bit-reversed, MSB first (= the constant as the reflected polynomial of the usual LSB-first algorithm) | all ones | none | yes / yes | upper-case UTF-16LE | **57** | **28** |
| 2 | same | same | same | same | as written | 2 | 2 (the identities that are already upper case) |
| all other 158 | | | | | | 0 | 0 |

Two published vectors agree (they are tests): `Microsoft.Windows.Explorer` gives `f01b4d95cf55d32a` (Explorer's own list), and
`{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe` gives `9b9cdc69c1c24e2b`.

**Which text is hashed.** The explicit AppUserModelID when the app has one (a packaged app's included). Without one, the program's
path in the shell's known-folder form (`{6D809377-6AF0-444B-8957-A3773F02200E}\Vendor\app.exe`: the longest matching known folder,
case-insensitive; the same string "All apps" uses as an id), and for a few apps the plain expanded path. Counts on this machine, over
the same 273 apps (an app can match under several): the id as written names an automatic file for 57 apps and a custom file for 28;
the known-folder form of the program's path names an automatic file for 5 apps and the plain path for 2. 62 of the 273 apps have an
automatic file and 28 a custom file under some candidate. (The rest are apps that were never used for a jump list, or whose file
carries an identity this list cannot reproduce, such as an app that sets its AppUserModelID at run time and is not running.)
`read` tries the candidates in that order and takes the first that names a file.

## Formats, as they were found

**DestList** (inside the compound file). Header 32 bytes: version, entries, pinned. On this machine all 78 non-empty lists are
**version 6** (the research note listed 1, 3 and 4): entries have the Windows 10 layout (entry number at 88, FILETIME at 100, pin
status at 108 where -1 is "not pinned", use count at 116, path length in characters at 128, the UTF-16LE path from 130), then a u32 that
is the size of an extra block (a serialized property store) and the block. The size is 0 in almost every entry and 2988 bytes in some;
reading it as "four bytes of padding" lost the entries after the first one with a block (one file of 104). Against all 104 files: 78
lists parse, 11 compound files have no DestList stream and 15 an empty one; 6593 entries parse, every one names a stream that
exists and has a path; in all 78 the header's entry count and pinned count equal the parsed ones.

**customDestinations**: `u32 version (2) | u32 categories | u32 0`, then per category `u32 type` (0 own, 1 known, 2 tasks), for type 0
a **u16** name length in characters and the name and `u32` entries, for type 1 the known category (1 Frequent, 2 Recent), for type 2
`u32` entries; then the entries (the shell link's CLSID and the link) and **a footer `0xBABFFBAB` after every category**, not
only the last. An entry runs to the next entry's CLSID-and-header; the last entry of a category runs to the footer, but only to a
footer that is followed by the end of the file or a category type (0 to 2), so that the four footer bytes inside a link's property
data do not end an entry (the link's own size field is not trusted). A file that declares more than 256 categories is not
"complete". Against all 465 files: 5 are empty, and the other 460 are read to their last footer with every category whole (657
categories, 2086 links; 422 files have Tasks, 116 their own categories, 6 list the Recent category and none the Frequent one).

**The shell links** load from memory (all 6593 + 2086 here) with `IShellLink`; title (`System.Title`), AppUserModelID and the
separator flag (`System.AppUserModel.IsDestListSeparator`) come from the link's property store, the target from its item ID list
(`IShellLink::GetPath`; a link with only a LinkInfo block gives no path, which is why the tests' fixtures carry an ID list).

## What Windows will not give

* Pinned items, the Tasks and the app's own categories through `IApplicationDocumentLists`: it returns Recent and Frequent only.
  They come from the files, which is why the AppID hash had to be settled.
* The order and the count Windows shows (it fits the list to the room it has); the caller cuts to its own room (`limit` per part).
* Anything for an app whose identity the dock cannot state (a window with neither an AppUserModelID nor a readable program).
* A change event: the files change when Explorer or an app writes them; there is no notification, so the caller reads again when it
  opens a menu.

## What is never written

Windows' stores are never written: no pin, no unpin, no removal of an item, no `RemoveAllDestinations`, no `ICustomDestinationList`
for another app. The reader opens files for reading only, asks the shell only to list (and only for apps that already have an
automatic file, so that nothing is created for an app that has none), and creates no file. The probe notes the size and
last-write time of every file in both folders before the reader part and after it, and prints how many differ (changed, added,
removed), split into the files of the apps it asked about and all the others: a reader that wrote would show in the first number
alone, the second is what Explorer and apps wrote meanwhile. On this machine both numbers were 0 in every row. That is evidence
for the `IApplicationDocumentLists` calls that were made (28 apps), not a proof that the call can never rewrite a list.

The one existing writer is `rf_win::jumplist::set` (rustyfiles' own list for its own AppUserModelID), unchanged; its unit test
writes a list for the test binary's identity, as it always did.

## Trying it

```
cargo run -p rf-win --example jumplist_probe              parts 1 to 3
cargo run -p rf-win --example jumplist_probe -- --no-reader   the file names and the parsers only
```

Windowless and read-only. It prints counts and timings (never an app's name, a title or a path): the sweep of the AppID hash, how
the parsers fare on every real file, and for each pinned and running app (numbered) the items in each part and the time the read took.
