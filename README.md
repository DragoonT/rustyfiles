<p align="center">
  <b>English</b> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.tr.md">Türkçe</a>
</p>

<img alt="rustyfiles: a file manager for Windows and macOS that never freezes. Written in Rust. Drawn as a rustyfiles window on a folder of 100,000 files, opened in 281 ms" src="docs/readme/cover.png" width="100%">

<p align="center">
  <b>A file manager that never waits on the disk.</b><br>
  Tabs, dual panes, archives that open like folders and a preview on <kbd>Space</kbd>.<br>
  Written from scratch in Rust, for Windows and now macOS.
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Latest version" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="Downloads" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11, x64 and ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <img alt="macOS 26 on Apple silicon and Intel, beta" src="https://img.shields.io/badge/macOS_26-Apple_silicon_and_Intel_(beta)-3d3d44?labelColor=232326">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Every release is scanned on VirusTotal: the reports are linked at the end of its notes" src="https://img.shields.io/badge/VirusTotal-every_release_scanned-3d3d44?labelColor=232326"></a>
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Download for Windows" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg"><img alt="Download for macOS (beta)" src="https://img.shields.io/badge/Download_for_macOS_(beta)-232326?style=for-the-badge&logo=apple&logoColor=white"></a>
  <br>
  <a href="https://rustyfiles.com"><img alt="Website" src="https://img.shields.io/badge/rustyfiles.com-232326?style=flat-square&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Join the Discord" src="https://img.shields.io/badge/Discord-5865F2?style=flat-square&logo=discord&logoColor=white"></a>
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="Buy me a coffee" src="https://img.shields.io/badge/Buy_me_a_coffee-FFDD00?style=flat-square&logo=buymeacoffee&logoColor=black"></a>
</p>

<table>
  <tr>
    <td width="33%" valign="top">
      <b>Never waits on the disk</b><br>
      Listing, icons, thumbnails, the context menu and every copy run off the window's thread.
      A slow network share or a folder of 100,000 files can't freeze it.
    </td>
    <td width="33%" valign="top">
      <b>Explorer's work, done faster</b><br>
      Tabs, dual panes, archives as folders, a name index for instant search, bulk rename, undo,
      and its own copy engine: 14 times faster on small files.
    </td>
    <td width="33%" valign="top">
      <b>Free and yours</b><br>
      No account, no ads, nothing sent anywhere. MIT licensed, every line here to read.
      Updates install quietly in the background.
    </td>
  </tr>
</table>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles showing a Projects folder in details view, with tabs, a breadcrumb bar and pinned folders in the sidebar; the window is glass over the desktop's wallpaper" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Windows, with the experimental HyperGlass backdrop (Settings › Appearance). The default is Mica, as in Explorer.</sub>

## How fast

<img alt="A speed test drawn as a rustyfiles list: opening a folder of 100,000 files takes 281 ms in rustyfiles and 2,318 ms in File Explorer (8 times faster); a folder of 2,000 pictures 278 ms against 2,177 ms (8 times); copying 10,000 small files 1.8 s against 25.6 s (14 times); extracting a ZIP of 10,000 files 3.6 s against 32.4 s (9 times)" src="docs/readme/speed.png" width="100%">

A folder of 100,000 files opens in 281 ms (File Explorer: 2.3 s). The numbers come from the speed test
built into the app, Settings › Speed test, so you can check them on your own PC.

<details>
<summary><b>Every measurement, and how it was taken</b></summary>

#### Opening a folder

Time from starting the app to the folder's list standing still, in a new window, with the folder
already in the OS file cache. Median of three runs after a warm-up.

| Folder | rustyfiles | File Explorer | [Files](https://files.community) |
|---|--:|--:|--:|
| Empty folder | 281 ms | 1,006 ms | 813 ms |
| 1,000 files | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 files | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 files | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 pictures (thumbnails) | 278 ms | 2,177 ms | 1,075 ms |
| Memory after listing 10,000 files | 39 MB | 69 MB | 204 MB |

Measured on one machine (Ryzen 7 9800X3D, NVMe SSD, Windows 11) on 2026-11-16, with rustyfiles
0.8.6 and Files 4.3. rustyfiles and Explorer each started a new process. Files was already running
in the background, so its times are not those of a cold start; it also fills its list in steps, and
the time counts until the picture stops changing. Memory is the private working set, the figure Task
Manager shows. With "keep running in the background" on, a new rustyfiles window starts no process
and showed a listed folder in 93–168 ms (2026-11-06, an earlier version).

#### A 100,000-file folder that keeps changing

The folder open, one file selected, then about 1,000 files renamed per second in the background
for 5 seconds:

| | |
|---|--:|
| Launch to a listed folder (100,001 files) | 315 ms |
| The list follows the changes | every ~750 ms |
| Re-reading the folder (background thread) | ~70 ms |
| Work on the UI thread per update | 7–8 ms |
| Selection and cursor | stay on the selected file |

Frame times while the same folder is scrolled down and back up for 7.7 s, the selection (and so the
preview pane) moving every half second, about 680 files a second renamed in the background, on a
200 Hz display:

| | median | p95 | p99 | worst |
|---|--:|--:|--:|--:|
| Frame cost on the UI thread | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| Time between frames | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

2,182 frames in 7.7 s; one gap longer than a 60 Hz frame. Measure it yourself with
`RUSTYFILES_TRACE_LIVE=1` and `RUST_LOG=warn,rf_app=info`, and log frames with
`GPUI_FRAME_LOG=frames.tsv`.

#### New tab in an open window

| Folder | Time to a settled list |
|---|--:|
| 1,000 files | ~48 ms |
| 10,000 files | ~46 ms |
| 100,000 files | ~129 ms |

Each extra tab costs about 3 MB of memory (1 tab: ~32 MB, 25 tabs: ~103 MB).

#### Search by file name

rustyfiles keeps a name index of the local drives and answers searches from it. Searching every
subfolder of a user folder with 3.6 million files and folders for "rapor", timed until the results
on screen stopped changing:

| | Time |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| File Explorer | 1.8 s (its fastest of 4 runs; the others took 11–32 s) |

The index stores each different name once: a drive with 4.8 million entries but 490 thousand
different names takes 61 MB. A folder watcher keeps it current while rustyfiles runs; between runs,
NTFS's change journal (read without administrator rights) says which folders changed, so a start
lists only those again: 0.2 s for C: here, where a walk takes 30 s. App data, caches,
`node_modules` and `.git` are left out; a search started inside one walks the disk:

| `*.dll` under `C:\Windows` (342,492 items, 116,851 folders, cache warm) | Time |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

It searches on six background-priority threads, so the rest of the machine stays responsive, and
doesn't follow junctions, which is why it finds 454 fewer duplicates.

#### Preview with Space (DirectLook)

| File | Content ready, from the key press |
|---|--:|
| JPEG, 3840 × 2160 | 24–86 ms |
| Source code, syntax-highlighted | 23–44 ms |
| 7z archive (contents listed) | 19–31 ms |
| MP3 (tags, lyrics) | 27–32 ms |
| H.264 video, 1080p | first frame in 146–185 ms |

#### Archives

Reading RAR with [rarchive](https://github.com/gelenmedya/rarchive), our own engine, against the
official UnRAR (decode and check every CRC, archives in the file cache):

| Archive | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB, 32 MB dictionary, 195 files | 2.79 s | **0.28 s** (10×) |
| 400 MB solid | 0.95 s | **0.21 s** (4.5×) |
| 400 MB, 520 files | 1.03 s | **0.063 s** (16×) |

#### Copying and extracting

Our own engines against Windows' (rustyfiles 0.8.6, 2026-11-16):

| | Windows | rustyfiles |
|---|--:|--:|
| Copying 10,000 small files in 100 folders (41 MB) | 25.6 s | **1.8 s** (14×) |
| Copying one 1 GB file | 0.61 s | 0.44 s |
| Extracting a ZIP of 10,000 files | 32.4 s | **3.6 s** (9×) |

Copying spreads small files across folders, because threads writing into one folder wait on each
other; a single large file is limited by the disk, not the engine. It handles name conflicts itself
(replace, skip or keep both), pauses and resumes, can verify every copy byte by byte and can cap its
speed. The copy test script is `crates/rf-win/examples/copybench.rs`.

All numbers: one machine (Ryzen 7 9800X3D, NVMe SSD, Windows 11), November 2026.

</details>

## What it does

- **Browsing.** Tabs you can drag out into a window, saved sessions, dual pane, Details, grid, list
  and column views, grouping, a preview pane, colour tags, the Shelf, network shares and FTP.
- **Files.** Undo and redo, bulk rename, folder sizes on demand, disk usage with a treemap, the full
  Explorer context menu, drag and drop with every other app.
- **Archives.** ZIP, 7z, RAR, tar and ISO open like folders; ZIP, 7z and tar can be edited inside.
- **Search.** A name index of your drives, and quick search (<kbd>Alt</kbd>+<kbd>Space</kbd>) for
  apps, files, bookmarks and Windows' settings.
- **Windows.** Optionally the default file manager, including <kbd>Win</kbd>+<kbd>E</kbd> and other
  apps' Open and Save dialogs.
- **49 languages**, following the system's.

<details>
<summary><b>Every feature in detail</b></summary>

**Browsing.** Tabs you can drag to reorder or pull out into a new window. Save a window's tabs,
split panes included, as a named session and open it again later. A new tab opens Home or the folder
you are in (Settings › General). Dual pane. Details, grid, list and column views (the list can fill
column by column, as Explorer's does), compact items like Explorer's compact view, grouping, a
preview pane, search, colour tags, the Shelf, libraries, network shares and FTP. A Git status
column. Folder sizes on demand: click "Calculate size"; they are remembered across restarts and can
show size on disk. "Show disk usage" on a folder or drive lists what takes the space, biggest first,
next to a treemap you can click through. Virtual folders such as Libraries and Gallery open in
rustyfiles, and devices Windows lists under This PC (phones over USB or Phone Link) appear under
Devices on the Home page. Cloud folders Windows knows (OneDrive, Proton Drive, Dropbox, MEGA and
others) are in the sidebar.

**Files.** Undo and redo for renames, copies, moves, deleting to the Recycle Bin and folder
flattening. Bulk rename. Duplicate, copy or move to the other pane, and paste copied text or a
picture as a new file. The full Explorer context menu with Send to, drag and drop with Explorer, the
desktop, browsers and Outlook, and drop targets on tabs, pinned folders, folders and programs. Send
files to your other computers with [Ferry](#ferry), and download with [HyperDownload](#hyperdownload).

**Archives.** ZIP, 7z, RAR, the tar family and ISO open like folders, password-protected ZIP, 7z
and RAR included. Add, delete and rename inside ZIP, 7z and tar. Create ZIP, 7z and tar.zst on every
core. Reading is done by [rarchive](https://github.com/gelenmedya/rarchive), our own pure-Rust
engine (also [on crates.io](https://crates.io/crates/rarchive)): it decodes a 1.3 GB WinRAR archive
ten times faster than UnRAR.

**Windows.** Optionally the default file manager: folders, <kbd>Win</kbd>+<kbd>E</kbd>, This PC
and the Recycle Bin open in rustyfiles. Optionally, other apps' Open, Save and Select folder dialogs
open in rustyfiles too (Settings › Advanced), and a pin button there keeps a start folder for each
app. Pictures and videos opened in Photos or another Store app come with their folder, so its
Previous and Next work. Explorer's `/select,` and `shell:` paths work. Mica, dark mode and the
system accent colour. When a game or a render saturates the GPU, drawing moves to the CPU and back
again without a flicker.

**Commands in the address bar.** As in Explorer: press <kbd>Ctrl</kbd>+<kbd>L</kbd>, type `cmd`,
`powershell`, `wt` or `code .` and press <kbd>Enter</kbd>. The text is first taken as a place (a
path, a drive, `%VAR%`, `shell:`); a folder or file of that name in the open folder always wins.
Only when there is no such place is the first word looked up as a program, the way the Run box does,
and started in the open folder. Nothing is started from a completion, and no administrator rights are
asked for. A switch under Settings › Files and folders › Behaviors turns it off.

**Organizing.** Rules for a folder: conditions on name, extension, kind, size and age, and one action
(move, move into a folder built from a template, rename, or the Recycle Bin). A run by hand shows
every step in a preview and moves nothing before Apply, and a run can be undone. Nothing is deleted
for good and nothing is overwritten. Rules that run by themselves are off by default.

**Search.** Quick search (<kbd>Alt</kbd>+<kbd>Space</kbd>) finds apps, files and folders, browser
bookmarks and Windows' settings pages. What you open often comes first, and what you chose for the
same letters before is the first row.

**Languages.** 49 of them, following the system's display language (or pick one in Settings ›
General). The interface is written in Turkish and English; the other languages started from the Files
community's wording and are machine-translated from there on. Right-to-left names and text read right
to left (thanks to @ialhabbal); the interface itself is not mirrored yet. Corrections are welcome:
each language is one JSON file in `crates/rf-app/locales/`.

</details>

<img alt="rustyfiles showing a folder of Windows' wallpapers in grid view, one of them selected and shown in the preview pane on the right, on the HyperGlass backdrop" src="docs/readme/grid.png" width="100%">

## DirectLook

Select a file and press <kbd>Space</kbd>: video and audio with their own player, images, text and
code, the contents of archives, and a details card for everything else. Arrow keys move through the
folder while it stays open. It runs as its own program, so a heavy video never weighs on the file
manager, and if you turn it on it works in Explorer and on the desktop too (Settings › DirectLook).

## Ferry

Send files and folders to rustyfiles on your other computers on the same network, the way AirDrop
does: right-click › **Send with Ferry**. Off until you turn it on in Settings › Ferry.

<img alt="Settings › Ferry with two nearby devices, a paired Windows PC with its measured speed and a MacBook with an estimated one; a Windows notification asks whether to accept files from the MacBook, and the status bar shows a transfer at 61 percent" src="docs/readme/ferry.png" width="100%">

<sub>A drawing of Settings › Ferry with invented devices, in the app's own words and layout.</sub>

<details>
<summary><b>How Ferry works</b></summary>

- Encrypted end to end (QUIC with TLS 1.3), straight from device to device: no server, no account,
  nothing leaves your network.
- Pair your devices the way you pair Bluetooth: both show the same six-digit code. A paired device's
  files can arrive without asking; anything else is asked first, in a notification.
- You choose who can see your computer: no one, your paired devices (the default), or everyone for
  ten minutes. While Settings › Ferry is open everyone can see it, so a new device can find it.
- A transfer that breaks off goes on from where it stopped, and every file is checked before it takes
  its name. Received files are marked as downloaded, so SmartScreen asks before a program among them
  runs.
- Each device shows an estimated speed, and **Test speed** measures it both ways in a few seconds.
- The macOS app speaks the same protocol, written down in [docs/nearby-protocol.md](docs/nearby-protocol.md).
  Windows Firewall may ask about Ferry: allow it for private networks.

</details>

## HyperDownload

rustyfiles' own download manager, an add-on: Settings › Add-ons › HyperDownload › **Install**. One
file over many connections at once, paused and resumed where it stopped, torrents and magnet links,
a Downloads page, and videos that play in DirectLook before they have finished. It is a separate
program ([source](https://github.com/gelenmedya/hyperdownload), MIT), downloaded only when you click
Install, checked against its SHA-256 first, and removable at any time.

## Experimental on Windows: HyperGlass, the Dock and the desktop

Off by default and early: built and tried on one PC with two monitors. Expect rough edges, and please
report what you find. Explorer keeps running underneath; nothing of Windows is removed or replaced on
disk.

<img alt="A desktop with the rustyfiles Dock at the bottom (three islands: Start and search, the pinned and running apps, the tray and the clock), the Dock's Start panel open on the left with pinned apps and recent files, HyperSearch in the middle with the apps found for adobe, and the control centre on the right with Wi-Fi, Bluetooth, Ethernet, the song playing, volume and brightness" src="docs/readme/shell.png" width="100%">

<sub>Start, HyperSearch and the control centre, put together from three screenshots of the same desktop (they don't open all at once).</sub>

<details>
<summary><b>What each part does</b></summary>

- **HyperGlass** (Settings › Appearance) is a fifth backdrop: the window's background is glass that
  bends and frosts what is behind it.
- **The Dock** (Settings › Dock, one switch) puts three glass islands at the bottom of each monitor
  in place of Windows' taskbar: a Start panel and search of its own, the running and pinned apps with
  window previews, the tray icons, a control centre and a notification panel. While it is on, a tap
  of the Windows key opens the Dock's Start and <kbd>Alt</kbd>+<kbd>Tab</kbd> opens its switcher.
  Switch the Dock off and Windows' own are back.
- **The desktop** drawn by rustyfiles comes with the Dock: your wallpaper and icons, with an option
  for rounded corners.
- **Brightness for every monitor.** Windows has a slider only for a laptop's own screen; the Dock's
  control centre has one for each monitor. External monitors are set over DDC/CI, which almost every
  desktop monitor of the last fifteen years speaks over DisplayPort, HDMI and DVI. If a monitor has
  no slider, check that DDC/CI is on in its own menu; some docks, KVM switches and adapters don't
  pass it through.

</details>

## macOS (beta)

The same file manager, drawn natively with AppKit and Liquid Glass over the same Rust core: tabs,
dual pane, every view, Quick Look on <kbd>Space</kbd>, archives as folders, bulk rename, disk usage,
the Shelf, colour tags, Git status, Connect to Server, Ferry and the same 49 languages. The Dock, the
desktop, HyperSearch and DirectLook stay on Windows for now, and HyperDownload's Mac build follows in
its own release. Apple silicon and Intel Macs with macOS 26 (Tahoe). It updates itself like the Windows app.

## Install

**Windows.** [Download rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)
(about 50 MB) and open it. If Windows says "Windows protected your PC", click **More info**, then
**Run anyway**: the installer isn't code-signed yet, and every release is scanned on
[VirusTotal](https://www.virustotal.com) (the reports are linked at the end of its release notes).
rustyfiles opens by itself and is in your Start menu. No administrator password; updates install
quietly in the background.

**macOS (beta).** [Download rustyfiles-mac.dmg](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg),
open it and drag rustyfiles to Applications. It is signed and notarized by Apple, so it opens like any
other Mac app.

<details>
<summary><b>Scoop, portable, ARM64, default file manager, uninstalling</b></summary>

**With [Scoop](https://scoop.sh):**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**Portable or ARM64:** a portable zip and an ARM64 installer (`rustyfiles-win-arm64-Setup.exe`) are
on the [Releases](https://github.com/gelenmedya/rustyfiles/releases) page. Windows 11 is required
(Windows 10 is untested). Linux is planned.

**Folders and <kbd>Win</kbd>+<kbd>E</kbd> in rustyfiles:** turn on Settings › Advanced › Set as
default file manager. You can turn it off any time.

**Uninstall** like any app, from Windows Settings › Apps. Everything rustyfiles changed goes back to
File Explorer.

**Open/Save dialogs in other apps fail or crash** after rustyfiles' files were removed without an
uninstall (a virus scanner, a broken update)? Starting rustyfiles again fixes it. If it no longer
starts, this PowerShell command gives every app Windows' own dialog back:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

</details>

<details>
<summary><b>Build from source</b></summary>

You need [Rust](https://rustup.rs) 1.95 or newer and the Visual Studio Build Tools with the
"Desktop development with C++" workload (one dependency, zstd, compiles C code).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # builds, then installs for your user
.\scripts\install.ps1 -Default   # also makes it the default file manager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` starts it without installing. The macOS app lives in `macos/` and
`crates/rf-mac`, outside the Windows workspace.

</details>

**Questions or ideas?** Come say hi on [Discord](https://discord.gg/K2banZ36zJ), or visit
[rustyfiles.com](https://rustyfiles.com).

## Under the hood

<details>
<summary><b>Keyboard</b></summary>

| Keys | Action |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | New tab, close tab, reopen closed tab |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> or <kbd>Alt</kbd>+<kbd>D</kbd> | Address bar |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Search (<kbd>Enter</kbd> searches subfolders too) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> or <kbd>F1</kbd> | Command palette |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | Find a folder you visited, find a tab |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | Details, grid, columns, cards, list |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>, <kbd>F6</kbd> | Dual pane, switch pane |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | Copy, move to the other pane |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | Duplicate |
| <kbd>Alt</kbd>+<kbd>P</kbd>, <kbd>Ctrl</kbd>+<kbd>B</kbd> | Preview pane, sidebar |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | Rename (bulk rename with several items selected) |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Undo, redo |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | Show hidden items |

Every shortcut can be changed in Settings › Actions: click a command, then press the new keys.

</details>

<details>
<summary><b>Project layout</b></summary>

| Crate | What it holds |
|---|---|
| `crates/rf-app` | The Windows interface, and its two programs: `rustyfiles` and `directlook` (`src/look/`) |
| `crates/rf-win` | Windows and shell integration, the file operation engine |
| `crates/rf-core` | The model shared by both platforms: locations, history, entries, formats |
| `crates/rf-archive` | Archive operations: browse, extract, create, edit |
| `crates/rf-nearby` | Ferry, shared with the macOS app |
| `crates/rf-player` | DirectLook's video and audio player |
| `crates/rustdecoder` | Our own video engine, work in progress: MP4/MOV and MKV/WebM; H.264, HEVC, VP9 and AV1 decoded on the graphics card through D3D11 Video, bit-exact with ffmpeg / dav1d in its tests; ProRes and DNxHR on the CPU; audio and A/V sync ([design](docs/design/rustdecoder.md)) |
| `macos/`, `crates/rf-mac` | The macOS app (AppKit) and its bridge to the Rust core |

The Windows interface is built with [GPUI](https://gpui.rs), the framework behind the Zed editor. Its
Windows layer is a fork that adds the GPU/CPU switch:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

</details>

<details>
<summary><b>Environment variables</b></summary>

| Variable | Effect |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` or `auto` | Force where the window is drawn |
| `RUSTYFILES_CONFIG_DIR` = folder | Keep settings in another folder (portable use) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Allow several copies at once (tests) |
| `RUSTYFILES_FRAME_LOG=1` | Log frame times |
| `RUSTYFILES_LOOK_TRACE=1` | Log DirectLook's timings to stderr |
| `RUSTYFILES_MUTE=1` | DirectLook plays without sound (tests) |

Settings are stored in `%APPDATA%\rustyfiles\settings.toml`, DirectLook's in `directlook.toml` next
to it.

</details>

<details>
<summary><b>How it is made</b></summary>

rustyfiles is written with AI coding tools, and that is no secret. One developer decides what gets
built and how it should behave, tries each change on a real PC (two monitors, everyday work) and
turns down what doesn't hold up. The AI writes much of the code; it doesn't decide what ships.

What keeps that honest:

- **Tests.** More than 4,000 automated tests run before every change is pushed.
- **Measurements.** The speed numbers come from a test built into the app (Settings › Speed test),
  so anyone can check them on their own machine.
- **Reports.** Bugs come from the people who use it. Each issue gets an answer, the fix is made in
  the open, and the person who reported it is asked to try it before it is closed.
- **The code.** All of it is here under the MIT license, to read and to judge.

Why Rust: a file manager has to stay fast with 100,000 files in a folder and must never lose anyone's
data. Rust gives speed without a garbage collector and rules out whole classes of crashes before they
ship. rustyfiles is a new app written from scratch, not a rewrite of Explorer; it runs next to
Explorer and changes nothing of Windows on disk.

</details>

## Status

rustyfiles is young, and its author uses it every day in place of Explorer. The macOS app is a first
beta on the same core, and Linux comes after it. Bug reports and pull requests are welcome.

## Listed on

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/), as an alternative to File Explorer
- [Awesome GPUI](https://github.com/zed-industries/awesome-gpui#productivity), in the Productivity section
- [Awesome Rust](https://github.com/rust-unofficial/awesome-rust#file-manager), in the File manager section
- [Scoop](https://scoop.sh), from its own bucket (see [Install](#install))

## Thanks

rustyfiles stands on other people's work, and borrowed a lot of ideas along the way.

[Files](https://github.com/files-community/Files) showed what a modern file manager for Windows can
be. Its feature set was our checklist, its settings and layout shaped ours, and its approach to
replacing Explorer as the default file manager guided ours. Thank you to the Files community.

[GPUI](https://gpui.rs), the UI framework of the [Zed](https://github.com/zed-industries/zed) editor,
draws every pixel of rustyfiles on Windows on the GPU and makes a window this fast possible. Thank you
to the Zed team for building it and sharing it, and to [Longbridge](https://github.com/longbridge/gpui-kit)
for gpui-kit, the components we build on.

DirectLook follows the idea of macOS Quick Look, and [QuickLook](https://github.com/QL-Win/QuickLook)
by Paddy Xu, which brought it to Windows first. rarchive learned the RAR format from the UnRAR source
by Alexander Roshal.

Built with [windows-rs](https://github.com/microsoft/windows-rs), [Velopack](https://velopack.io)
(install and updates), [Lucide](https://lucide.dev) icons, [zip](https://github.com/zip-rs/zip2),
[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust), [tar](https://github.com/alexcrichton/tar-rs),
[notify](https://github.com/notify-rs/notify) and many more crates. The pictures use
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage) by Atelier Triay,
[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) and
[Doto](https://fonts.google.com/specimen/Doto).

## License

MIT, see [LICENSE](LICENSE). The GPUI fork keeps Zed's Apache-2.0 license in its own repository.
