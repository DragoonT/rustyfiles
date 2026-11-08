<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/banner-dark.svg">
  <img alt="rustyfiles: a file manager for Windows that never freezes. Written in Rust." src="docs/readme/banner-light.svg" width="100%">
</picture>

rustyfiles is a file manager for Windows, written from scratch in Rust. It is meant to replace
File Explorer for everyday work: tabs, dual panes, archives you open like folders, and a
Quick Look–style previewer called DirectLook.

The window never waits on the disk. Listing, icons, thumbnails, the context menu and every file
operation run off the UI thread, so a slow network share or a folder with 100,000 files can't
freeze it.

Download the latest build from [Releases](https://github.com/gelenmedya/rustyfiles/releases),
or [build it yourself](#build-from-source).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles showing a Projects folder in details view, with tabs, a breadcrumb bar and pinned folders in the sidebar" src="docs/readme/projects-light.png" width="100%">
</picture>

## How fast

Time from launching the app to the folder list settling, in a new window, with the folder
already in the OS file cache. Median of five runs.

| Folder | rustyfiles | File Explorer | [Files](https://files.community) |
|---|--:|--:|--:|
| 1,000 files | 93 ms | 1,113 ms | 2,640 ms |
| 10,000 files | 110 ms | 1,065 ms | 3,053 ms |
| 100,000 files | 168 ms | 1,186 ms | 11,342 ms |
| Memory after listing 10,000 files | ~40 MB | ~70 MB | ~250 MB |

Measured on one machine (Ryzen 7 9800X3D, NVMe SSD, Windows 11) on 2026-11-06. rustyfiles was
already running in the background; Explorer and Files each started a new process, and Files
loads its list incrementally, so its numbers are an upper bound. Starting rustyfiles cold
takes 350–480 ms.

Copying uses its own parallel engine: 16 times faster than Explorer's on 520 files (381 MB),
about 30 times on 20,000 small files. Name conflicts, the Recycle Bin and links still go through
Windows' own engine.

## What it does

**Browsing.** Tabs you can drag to reorder or pull out into a new window. Dual pane. Details,
grid and column views, grouping, a preview pane, search, colour tags, the Shelf, libraries,
network shares and FTP. A Git status column. Folder sizes on demand: click "Calculate size".

**Files.** Undo and redo for renames, copies, moves, deleting to the Recycle Bin and folder
flattening. Bulk rename.
The full Explorer context menu, drag and drop with Explorer and the desktop, and drop targets
on tabs and pinned folders.

**Archives.** ZIP, 7z, RAR, the tar family and ISO open like folders, password-protected
ZIP, 7z and RAR included. Add, delete and rename inside ZIP, 7z and tar. Create ZIP, 7z and tar.zst on every core. Reading is
done by [rarchive](https://github.com/gelenmedya/rarchive), our own pure-Rust engine: it
decodes a 1.3 GB WinRAR archive ten times faster than UnRAR.

**Windows.** Optionally the default file manager: folders, <kbd>Win</kbd>+<kbd>E</kbd>, This PC
and the Recycle Bin open in rustyfiles. Optionally, other apps' Open, Save and Select folder
dialogs open in rustyfiles too (Settings › Advanced). Explorer's `/select,` and `shell:` paths
work. Mica,
dark mode and the system accent colour. When a game or a render saturates the GPU, drawing
moves to the CPU and back again without a flicker.

**Languages.** 46 of them, following the Windows display language (or pick one in Settings ›
General). Translations start from the Files community's wording; Arabic, Hebrew and Persian are
translated but wait for right-to-left text in GPUI. Corrections are welcome: each language is one
JSON file in `crates/rf-app/locales/`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/photos-dark.png">
  <img alt="rustyfiles showing a Photos folder in grid view with image thumbnails" src="docs/readme/photos-light.png" width="100%">
</picture>

## DirectLook

Select a file and press <kbd>Space</kbd>. DirectLook opens a preview window: video and audio
(with its own player), images, text and code, the contents of archives, and a details card
for everything else. Arrow keys move through the folder while it stays open.

It runs as its own program, `directlook.exe`, so a heavy video never weighs on the file
manager and neither can take the other down. It works in rustyfiles, and if you turn it on,
in Explorer and on the desktop too. Settings live under Settings › DirectLook.

## Install

Download **rustyfiles-win-Setup.exe** from
[Releases](https://github.com/gelenmedya/rustyfiles/releases) and run it. It installs for your
user only (no administrator rights), adds rustyfiles to the Start menu and starts it. New
versions download in the background and apply at the next start; Settings › About shows the
version and checks on demand.

To open folders and <kbd>Win</kbd>+<kbd>E</kbd> in rustyfiles, turn on Settings › Advanced ›
Set as default file manager. Uninstall from Windows Settings › Apps: everything rustyfiles
changed in Windows goes back to Explorer.

The setup isn't code-signed yet, so Windows SmartScreen may warn on first run: choose
"More info", then "Run anyway". A portable zip is on the same page.

Requires Windows 11 on x64. Windows 10 is untested.

### Build from source

You need [Rust](https://rustup.rs) 1.95 or newer and the Visual Studio Build Tools with the
"Desktop development with C++" workload (one dependency, zstd, compiles C code).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # builds, then installs for your user
.\scripts\install.ps1 -Default   # also makes it the default file manager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` starts it without installing.

## Keyboard

| Keys | Action |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | New tab, close tab, reopen closed tab |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> or <kbd>Alt</kbd>+<kbd>D</kbd> | Address bar |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Search (<kbd>Enter</kbd> searches subfolders too) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> or <kbd>F1</kbd> | Command palette |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> / <kbd>2</kbd> / <kbd>3</kbd> | Details, grid, columns |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>, <kbd>F6</kbd> | Dual pane, switch pane |
| <kbd>Alt</kbd>+<kbd>P</kbd>, <kbd>Ctrl</kbd>+<kbd>B</kbd> | Preview pane, sidebar |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | Rename (bulk rename with several items selected) |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Undo, redo |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | Show hidden items |

Every shortcut can be changed in Settings › Keyboard shortcuts: click a command, then press
the new keys.

## Project layout

| Crate | What it holds |
|---|---|
| `crates/rf-app` | The interface, and the two programs: `rustyfiles` and `directlook` (`src/look/`) |
| `crates/rf-win` | Windows and shell integration, the file operation engine |
| `crates/rf-core` | The model: locations, history, entries |
| `crates/rf-archive` | Archive operations: browse, extract, create, edit |
| `crates/rf-player` | DirectLook's video and audio player |

The interface is built with [GPUI](https://gpui.rs), the framework behind the Zed editor. Its
Windows layer is a fork that adds the GPU/CPU switch:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

<details>
<summary>Environment variables</summary>

| Variable | Effect |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` or `auto` | Force where the window is drawn |
| `RUSTYFILES_CONFIG_DIR` = folder | Keep settings in another folder (portable use) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Allow several copies at once (tests) |
| `RUSTYFILES_FRAME_LOG=1` | Log frame times |
| `RUSTYFILES_LOOK_TRACE=1` | Log DirectLook's timings to stderr |
| `RUSTYFILES_MUTE=1` | DirectLook plays without sound (tests) |

Settings are stored in `%APPDATA%\rustyfiles\settings.toml`, DirectLook's in
`directlook.toml` next to it.

</details>

## Status

rustyfiles is young, and its author uses it every day in place of Explorer. A roadmap and
test reports will follow in `docs/`.

Bug reports and pull requests are welcome.

## Thanks

rustyfiles stands on other people's work, and borrowed a lot of ideas along the way.

[Files](https://github.com/files-community/Files) showed what a modern file manager for
Windows can be. Its feature set was our checklist, its settings and layout shaped ours, and its
approach to replacing Explorer as the default file manager guided ours. Thank you to the Files
community.

[GPUI](https://gpui.rs), the UI framework of the [Zed](https://github.com/zed-industries/zed)
editor, draws every pixel of rustyfiles on the GPU and makes a window this fast possible.
Thank you to the Zed team for building it and sharing it, and to
[Longbridge](https://github.com/longbridge/gpui-kit) for gpui-kit, the components we build on.

DirectLook follows the idea of macOS Quick Look, and
[QuickLook](https://github.com/QL-Win/QuickLook) by Paddy Xu, which brought it to Windows first.
rarchive learned the RAR format from the UnRAR source by Alexander Roshal.

Built with [windows-rs](https://github.com/microsoft/windows-rs),
[Velopack](https://velopack.io) (install and updates), [Lucide](https://lucide.dev) icons,
[zip](https://github.com/zip-rs/zip2), [sevenz-rust2](https://github.com/hasenbanck/sevenz-rust),
[tar](https://github.com/alexcrichton/tar-rs), [notify](https://github.com/notify-rs/notify)
and many more crates. The banner uses
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage) by Atelier Triay.

## License

MIT, see [LICENSE](LICENSE). The GPUI fork keeps Zed's Apache-2.0 license in its own
repository.
