//! Drag and drop in the Start panel: the rules. Pure, except the few functions that say they read the disk or ask the shell (and
//! they are for a worker thread). The view that puts them on the screen is `rf-app/src/dock/start_drag.rs`.
//!
//! WHAT CAN BE PICKED UP ([`Item`]):
//! * a PINNED APP (a tile of "Sabitlenenler"): dragged inside the grid it is reordered; let go over the dock's middle island it is
//!   added to the dock's own pins; taken out of the panel anywhere else it becomes a real file drag of the app's Start-menu shortcut
//!   (only an app that has one: a packaged app has no such file and cannot be dropped anywhere but on the dock).
//! * an APP OF "TÜMÜ" (a row of the all-apps list): the same, without the grid.
//! * a RECOMMENDED FILE (an entry of "Önerilenler"): a real file drag from the first moment the press has moved, so that it can be
//!   dropped into any program, a folder window or the desktop. The target decides what a drop means; nothing here moves a file.
//!
//! A press that does not move more than the system's drag distance stays a click ([`moved_enough`]).
//!
//! WHAT IS NOT BUILT: dropping at a CHOSEN PLACE among the dock's icons (a drop on the island appends to the end of the dock's pinned
//! part), a drag from a result of the search, and moving a pin to another PAGE of the grid (the drop slots are the cells of the page on
//! the screen).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use super::layout::Rect;
use super::model::{DockApp, IconSource, Launch, PinnedApp, looks_packaged};
use super::ownpins::{OwnPin, PinFile};
use super::startpins::Pin;
use crate::apps::App;

// ---------------------------------------------------------------- click or drag

/// How far the pointer must move from the press before it is a drag: Windows' own drag distance (`SM_CXDRAG`, `SM_CYDRAG`, 4 px by
/// default, in device px), at least 1.
pub fn drag_distance() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXDRAG, SM_CYDRAG};
    let (x, y) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
    (x.max(1), y.max(1))
}

/// Whether a press that has moved by (`dx`, `dy`) is a drag and not a click. `limit` is the drag distance in the same unit as the
/// movement. MORE than the distance in either direction, as Windows' own rectangle around the press is meant.
pub fn moved_enough(dx: f32, dy: f32, limit: (f32, f32)) -> bool {
    dx.abs() > limit.0 || dy.abs() > limit.1
}

/// Whether a drag that has lost its pointer should end. A drag normally ends with the release or with a move that has no button down;
/// this is the backstop for a pointer that went elsewhere (another window took the capture) and for which neither came. It looks at the
/// left button from time to time and calls the pointer lost only when the button looks up several looks in a row AND no mouse event has
/// been heard for a while. The second condition matters for touch, pen and remote input: they send mouse messages without setting the
/// physical button state that the look reads, so while their events keep coming the events decide, not the look.
#[derive(Clone, Copy, Debug, Default)]
pub struct LostPointer {
    looks: u8,
}

impl LostPointer {
    /// Looks in a row at a button that is up before the pointer is called lost.
    pub const LOOKS: u8 = 3;
    /// How long no mouse event may have been heard (ms) before a button that looks up counts.
    pub const QUIET_MS: u64 = 400;

    /// One look. `down`: the left button is down now. `quiet_ms`: how long ago the last mouse event of the drag was heard. True: lost.
    pub fn look(&mut self, down: bool, quiet_ms: u64) -> bool {
        if down || quiet_ms < Self::QUIET_MS {
            self.looks = 0;
            return false;
        }
        self.looks = self.looks.saturating_add(1);
        self.looks >= Self::LOOKS
    }
}

/// Whether the click that follows a press may do its work. The release that ends a drag is also a click on whatever lies under it and
/// must do nothing; a new press forgets the last drag's silenced click, so that the next real click works.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClickGate {
    silenced: bool,
}

impl ClickGate {
    /// A drag ended (dropped, cancelled or handed over): the release that comes is not a click.
    pub fn drag_ended(&mut self) {
        self.silenced = true;
    }

    /// A new primary press.
    pub fn pressed(&mut self) {
        self.silenced = false;
    }

    /// A click arrived: whether it may act. A silenced click is used up by this call.
    pub fn allow(&mut self) -> bool {
        !std::mem::take(&mut self.silenced)
    }
}

// ---------------------------------------------------------------- what is picked up, and where it is

/// What was picked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A tile of the pinned grid.
    Pinned,
    /// A row of the all-apps list.
    Listed,
    /// An entry of "Önerilenler" (a file).
    Recommended,
}

/// The pinned grid as it stands on the screen: the cells of the page that is shown. Panel pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// The top left corner of the first cell.
    pub origin: (f32, f32),
    /// One cell's width and height.
    pub cell: (f32, f32),
    pub cols: usize,
    pub rows: usize,
    /// The index (among the pins that are shown) of the first tile of this page, and how many tiles the page has.
    pub first: usize,
    pub count: usize,
}

impl Grid {
    /// The drop slot under a point: the index among the shown pins of the tile whose cell it is over; a cell beyond the last tile (the
    /// empty end of the last row) is the last tile's slot, so a drop there means "last". `None` outside the grid's rectangle.
    pub fn slot_at(&self, p: (f32, f32)) -> Option<usize> {
        if self.count == 0 || self.cols == 0 || self.cell.0 <= 0.0 || self.cell.1 <= 0.0 {
            return None;
        }
        let (x, y) = (p.0 - self.origin.0, p.1 - self.origin.1);
        if x < 0.0 || y < 0.0 || x >= self.cell.0 * self.cols as f32 || y >= self.cell.1 * self.rows as f32 {
            return None;
        }
        let (col, row) = ((x / self.cell.0) as usize, (y / self.cell.1) as usize);
        Some(self.first + (row * self.cols + col).min(self.count - 1))
    }

    /// The cell (column, row) of the tile at page position `local`, in cells.
    pub fn cell_of(&self, local: usize) -> (f32, f32) {
        let cols = self.cols.max(1);
        ((local % cols) as f32, (local / cols) as f32)
    }
}

/// Where the pointer is during a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Over a cell of the pinned grid: the slot (an index among the shown pins).
    Grid(usize),
    /// In the panel, not over the grid.
    Panel,
    /// Over the dock's middle island.
    Island,
    /// Anywhere else.
    Outside,
}

/// What [`where_is`] needs to know.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    /// The panel window's size, panel pixels.
    pub viewport: (f32, f32),
    /// The grid, when the home page is shown.
    pub grid: Option<Grid>,
    /// The dock's middle island, screen device px.
    pub island: Option<Rect>,
}

/// Where the pointer is. `panel` is the pointer in panel pixels (they run on outside the window while the window holds the mouse),
/// `screen` the same pointer in screen device px. The panel wins over the island where they overlap (they do not on a real screen).
pub fn where_is(panel: (f32, f32), screen: (i32, i32), scene: &Scene) -> Where {
    let inside = panel.0 >= 0.0 && panel.1 >= 0.0 && panel.0 < scene.viewport.0 && panel.1 < scene.viewport.1;
    if inside {
        return scene.grid.and_then(|g| g.slot_at(panel)).map_or(Where::Panel, Where::Grid);
    }
    if scene.island.is_some_and(|r| r.contains(screen.0, screen.1)) {
        return Where::Island;
    }
    Where::Outside
}

/// What letting go does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    /// The pin goes to this slot of the grid.
    Reorder { to: usize },
    /// The app is added to the dock's own pins (at the end of the pinned part).
    PinToDock,
    /// A real file drag starts (an app's shortcut, a recommended file): the target decides.
    HandToSystem,
    /// Everything stays as it was.
    Nothing,
    /// There is no such drop, and the pointer says so.
    NotAllowed,
}

/// What each kind of item may be dropped on. `from` is the pin's own slot (for a pinned app); `system_ok` is whether the item can
/// become a file drag (an app with a shortcut file in the Start menu's folders; a recommended file always).
pub fn drop_for(item: Item, at: Where, from: Option<usize>, system_ok: bool) -> Drop {
    match (item, at) {
        // a file is a real drag from the first moment
        (Item::Recommended, _) => Drop::HandToSystem,
        (Item::Pinned, Where::Grid(slot)) => if from == Some(slot) { Drop::Nothing } else { Drop::Reorder { to: slot } },
        (Item::Listed, Where::Grid(_)) | (_, Where::Panel) => Drop::Nothing,
        (_, Where::Island) => Drop::PinToDock,
        (_, Where::Outside) => if system_ok { Drop::HandToSystem } else { Drop::NotAllowed },
    }
}

/// Whether a point (screen device px; `None` when the pointer could not be read) is over the dock's middle island. For a drag the
/// SYSTEM runs (an app's shortcut taken out of the panel): the island is lit while the pointer is over it, and a drop that ends there is
/// a drop on the dock.
pub fn dropped_on_island(at: Option<(i32, i32)>, island: Option<Rect>) -> bool {
    at.zip(island).is_some_and(|((x, y), r)| r.contains(x, y))
}

/// Whether the window under the pointer (its top-level window) is the island's own. The island's rectangle alone is not enough for a
/// drop the SYSTEM ran: where the island is thin or its window has a region, a point inside the rectangle may belong to another
/// program's window, which then received the file; the app must not be pinned as well. `None` on either side is "no".
pub fn is_island_window(window_at: Option<isize>, island: Option<isize>) -> bool {
    matches!((window_at, island), (Some(w), Some(i)) if w == i)
}

/// What the pointer and the dock show while an item is over a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// The ordinary pointer.
    Plain,
    /// The dock's middle island takes the drop: it is highlighted.
    DockTakes,
    /// The "cannot be dropped here" pointer.
    NotAllowed,
}

pub fn feedback(item: Item, at: Where, from: Option<usize>, system_ok: bool) -> Feedback {
    match drop_for(item, at, from, system_ok) {
        Drop::PinToDock => Feedback::DockTakes,
        Drop::NotAllowed => Feedback::NotAllowed,
        _ => Feedback::Plain,
    }
}

// ---------------------------------------------------------------- the order of the pins

/// The order of `len` tiles after the tile `from` is dropped at slot `to`: `order[cell]` is the tile (by its old position) that stands
/// in that cell. The others make room: those between shift by one place. An out-of-range `from` or `to` is the last place, nothing at
/// all for an empty list.
pub fn move_to(len: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if len == 0 || from >= len {
        return order;
    }
    let to = to.min(len - 1);
    let tile = order.remove(from);
    order.insert(to, tile);
    order
}

/// The cells the tiles stand in while the tile `from` is held over slot `over` (`None`: not over the grid, nothing makes room):
/// `cells[tile]` is the tile's cell. The held tile's own entry is the cell it would land in.
pub fn cells_while_held(len: usize, from: usize, over: Option<usize>) -> Vec<usize> {
    let order = match over {
        Some(to) => move_to(len, from, to),
        None => (0..len).collect(),
    };
    let mut cells = vec![0; len];
    for (cell, &tile) in order.iter().enumerate() {
        cells[tile] = cell;
    }
    cells
}

/// The slot of a shown pin as a position on the page that is on the screen (`first`: the first slot of the page, `count`: its tiles).
/// `None` for a pin of another page.
pub fn local_slot(slot: usize, first: usize, count: usize) -> Option<usize> {
    slot.checked_sub(first).filter(|l| *l < count)
}

/// A drop slot as a position on the page, kept inside it (a slot of an earlier page is the first tile, of a later one the last).
pub fn clamped_local_slot(slot: usize, first: usize, count: usize) -> usize {
    slot.saturating_sub(first).min(count.saturating_sub(1))
}

/// The positions in the pin file of the pins whose app is among `apps`: the pins the panel shows, in the file's order. The same
/// choice as `PinFile::resolve` makes for the grid, so a slot of the grid is an index into this list.
pub fn shown_in(file: &super::startpins::PinFile, apps: &[App]) -> Vec<usize> {
    shown_positions(&file.pins, |p| apps.iter().any(|a| super::startpins::same_id(&a.id, &p.id)))
}

/// The positions in the pin file of the pins that are shown (those whose app is installed), in the file's order.
pub fn shown_positions(pins: &[Pin], installed: impl Fn(&Pin) -> bool) -> Vec<usize> {
    pins.iter().enumerate().filter(|(_, p)| installed(p)).map(|(i, _)| i).collect()
}

/// The new order of the pin FILE after the shown pin `from` is dropped at the shown slot `to`: `new[i]` is the old position of the pin
/// that now stands at `i`. The shown pins change places among the places they occupy; a pin of an app that is not installed (kept in
/// the file, not shown) stays exactly where it was.
pub fn apply_move(file_len: usize, shown: &[usize], from: usize, to: usize) -> Vec<usize> {
    let mut out: Vec<usize> = (0..file_len).collect();
    if from >= shown.len() {
        return out;
    }
    let order = move_to(shown.len(), from, to);
    for (cell, &tile) in order.iter().enumerate() {
        if let (Some(&place), Some(&old)) = (shown.get(cell), shown.get(tile))
            && place < file_len
            && old < file_len
        {
            out[place] = old;
        }
    }
    out
}

/// The pins in a new order (`perm` from [`apply_move`]). A permutation that is not one (a wrong length, a repeat) changes nothing.
pub fn permuted(pins: &[Pin], perm: &[usize]) -> Vec<Pin> {
    let mut seen = vec![false; pins.len()];
    let ok = perm.len() == pins.len()
        && perm.iter().all(|&i| {
            let fresh = i < seen.len() && !seen[i];
            if fresh {
                seen[i] = true;
            }
            fresh
        });
    if !ok {
        return pins.to_vec();
    }
    perm.iter().map(|&i| pins[i].clone()).collect()
}

// ---------------------------------------------------------------- the slide

/// How long the other tiles take to make room.
pub const SLIDE_MS: u32 = 120;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Slide {
    from: (f32, f32),
    to: (f32, f32),
    started_ms: u64,
}

/// The tiles' places while they slide: each goes from where it is drawn to its new cell as the dock's spring (`spring::curve`)
/// over [`SLIDE_MS`] (at once when the animations are off); a new target in the middle of a slide starts from where the tile is, so
/// nothing jumps. At rest nothing is asked of it: `settled` is true and no frame is wanted.
#[derive(Clone, Debug, PartialEq)]
pub struct Slides {
    tiles: Vec<Slide>,
    duration_ms: u32,
}

impl Slides {
    /// Every tile at rest in its cell. `duration_ms` is [`SLIDE_MS`], or 0 for no animation.
    pub fn at_rest(cells: &[(f32, f32)], duration_ms: u32) -> Slides {
        Slides { tiles: cells.iter().map(|&c| Slide { from: c, to: c, started_ms: 0 }).collect(), duration_ms }
    }

    fn pos_of(&self, s: &Slide, now_ms: u64) -> (f32, f32) {
        let elapsed = now_ms.saturating_sub(s.started_ms) as f32;
        if self.duration_ms == 0 || elapsed >= self.duration_ms as f32 {
            return s.to;
        }
        let t = super::spring::curve(elapsed / self.duration_ms as f32);
        (s.from.0 + (s.to.0 - s.from.0) * t, s.from.1 + (s.to.1 - s.from.1) * t)
    }

    /// Where tile `i` is drawn at `now_ms`, in cells.
    pub fn pos(&self, i: usize, now_ms: u64) -> (f32, f32) {
        self.tiles.get(i).map_or((0.0, 0.0), |s| self.pos_of(s, now_ms))
    }

    /// New cells for the tiles; only a tile whose cell changed starts a slide (from where it is drawn now).
    pub fn retarget(&mut self, cells: &[(f32, f32)], now_ms: u64) {
        if cells.len() != self.tiles.len() {
            *self = Slides::at_rest(cells, self.duration_ms);
            return;
        }
        for (i, &cell) in cells.iter().enumerate() {
            let s = self.tiles[i];
            if s.to != cell {
                let here = self.pos_of(&s, now_ms);
                self.tiles[i] = Slide { from: here, to: cell, started_ms: now_ms };
            }
        }
    }

    /// Every tile has arrived: no more frames are needed.
    pub fn settled(&self, now_ms: u64) -> bool {
        self.duration_ms == 0 || self.tiles.iter().all(|s| s.from == s.to || now_ms.saturating_sub(s.started_ms) >= self.duration_ms as u64)
    }
}

// ---------------------------------------------------------------- the app's shortcut file in the Start menu

/// The folders of the Start menu's programs: the all-users one and the user's own.
pub fn start_menu_dirs() -> Vec<PathBuf> {
    ["ProgramData", "APPDATA"].iter().filter_map(std::env::var_os).map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs")).collect()
}

/// A shortcut file of the Start menu and the name Windows shows for it when that is not its file name (a folder's `desktop.ini` can give
/// its shortcuts other names, in the user's language: the file `Notepad.lnk` is shown as "Not Defteri").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    pub shown: Option<String>,
}

fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The shortcut that stands for the app named `name`: the file name without `.lnk`, or the name Windows shows for it, compared without
/// regard to case; the last one of `files` that has that name (the user's own folder is listed after the all-users one and wins). `None`
/// when there is none.
pub fn pick_shortcut(files: &[Listed], name: &str) -> Option<PathBuf> {
    let want = fold(name);
    if want.is_empty() {
        return None;
    }
    files
        .iter()
        .rev()
        .find(|f| f.path.file_stem().is_some_and(|s| fold(&s.to_string_lossy()) == want) || f.shown.as_deref().is_some_and(|n| fold(n) == want))
        .map(|f| f.path.clone())
}

/// The `file=value` lines of the `[LocalizedFileNames]` section of a folder's `desktop.ini`: the shortcut's file name (as written) and
/// the name it is shown as, either plain text or a resource reference (`@%SystemRoot%\system32\shell32.dll,-22042`) that `resolve` turns
/// into text. Names `resolve` cannot give are left out.
pub fn localized_names(ini: &str, resolve: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut in_section = false;
    let mut out = Vec::new();
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[LocalizedFileNames]");
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((file, value)) = line.split_once('=') else { continue };
        let (file, value) = (file.trim(), value.trim());
        if file.is_empty() || value.is_empty() {
            continue;
        }
        let shown = if value.starts_with('@') { resolve(value) } else { Some(value.to_string()) };
        if let Some(shown) = shown.filter(|s| !s.trim().is_empty()) {
            out.push((file.to_string(), shown));
        }
    }
    out
}

/// A `desktop.ini` as text: Windows writes them in UTF-16 with a byte order mark, older ones in an ANSI code page (read as UTF-8, which is
/// right for the section names and the plain ASCII file names the names are looked up by).
fn ini_text(bytes: &[u8]) -> String {
    if let [0xFF, 0xFE, rest @ ..] = bytes {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// A resource string reference (`@file,-id`) as text, by the shell's own loader. No COM, any thread.
fn resolve_resource(reference: &str) -> Option<String> {
    use windows::Win32::UI::Shell::SHLoadIndirectString;
    use windows::core::PCWSTR;
    let from = crate::wide::wide(reference);
    let mut buf = [0u16; 260];
    unsafe { SHLoadIndirectString(PCWSTR(from.as_ptr()), &mut buf, None).ok()? };
    Some(crate::wide::from_wide(&buf)).filter(|s| !s.is_empty())
}

fn collect_shortcuts(dir: &Path, depth: u32, out: &mut Vec<Listed>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut here: Vec<Listed> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => collect_shortcuts(&p, depth - 1, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) => here.push(Listed { path: p, shown: None }),
            _ => {}
        }
    }
    // the names this folder's desktop.ini gives its shortcuts (read only when the folder has shortcuts)
    if !here.is_empty()
        && let Ok(bytes) = std::fs::read(dir.join("desktop.ini"))
    {
        let names = localized_names(&ini_text(&bytes), resolve_resource);
        for l in &mut here {
            let file = l.path.file_name().map(|n| n.to_string_lossy().to_lowercase());
            l.shown = names.iter().find(|(f, _)| Some(f.to_lowercase()) == file).map(|(_, s)| s.clone());
        }
    }
    out.extend(here);
}

/// How long the list of the Start menu's shortcuts is kept: it changes only when something is installed.
const SHORTCUTS_KEPT: Duration = Duration::from_secs(60);

/// The shortcut files of the Start menu (nothing is opened or resolved), kept for a minute. Reads the disk: a worker thread.
fn shortcut_files() -> Vec<Listed> {
    static CACHE: Mutex<Option<(Instant, Vec<Listed>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, v)) = c.as_ref()
        && at.elapsed() < SHORTCUTS_KEPT
    {
        return v.clone();
    }
    let mut out = Vec::new();
    for d in start_menu_dirs() {
        collect_shortcuts(&d, 4, &mut out);
    }
    *c = Some((Instant::now(), out.clone()));
    out
}

/// The Start-menu shortcut of an app, if it has one. A packaged app has none (it is started by its AppUserModelID, there is no file to
/// hand to a folder or the desktop), and a classic app whose shortcut cannot be found by its name has none either: such an app can be
/// dropped on the dock only. Reads the disk: a worker thread.
pub fn shortcut_of(app: &App) -> Option<PathBuf> {
    if looks_packaged(&app.id) {
        return None;
    }
    pick_shortcut(&shortcut_files(), &app.name)
}

/// Copies a shortcut into a folder of its own under `root` and returns the copy, named as the original. THE DRAG CARRIES THE COPY: a
/// target that moves what it is given (a same-drive drop in a program that moves by default) would otherwise take the shortcut out of
/// the Start menu, and a move from the all-users folder asks for administrator rights. The copy is the target's to move or copy; the
/// original is never touched. Old copies are removed by [`sweep`].
pub fn drag_copy(link: &Path, root: &Path) -> std::io::Result<PathBuf> {
    let name = link.file_name().ok_or_else(|| std::io::Error::other("a shortcut with no name"))?;
    // (a folder of its own for each drag: two drags of the same app never meet, and the file keeps its real name)
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = root.join(format!("{stamp:x}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let to = dir.join(name);
    if let Err(e) = std::fs::copy(link, &to) {
        // (no empty folder is left behind by a shortcut that was not there)
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    Ok(to)
}

/// Removes the drag copies under `root` that are older than `older_than` (a target may still be copying a recent one). Returns how many
/// folders went. Only folders directly under `root` are looked at.
pub fn sweep(root: &Path, older_than: Duration) -> usize {
    let Ok(rd) = std::fs::read_dir(root) else { return 0 };
    let mut gone = 0;
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= older_than);
        if old && e.file_type().is_ok_and(|t| t.is_dir()) && std::fs::remove_dir_all(e.path()).is_ok() {
            gone += 1;
        }
    }
    gone
}

// ---------------------------------------------------------------- an app on the dock

/// What the dock keeps for an app of the Start panel that is dropped on it. A packaged app is its AppUserModelID; a classic app is its
/// Start-menu shortcut when it has one (read on the shell thread), else its program as the shell names it. `None` when nothing can start
/// it again (a pin that does nothing is no pin). Blocks (COM, the shell): a worker thread.
pub fn dock_pin_of(app: &App, shortcut: Option<&Path>) -> Option<PinnedApp> {
    if looks_packaged(&app.id) {
        return Some(PinnedApp {
            name: app.name.clone(),
            aumid: Some(app.id.to_lowercase()),
            exe: None,
            launch: Launch::Packaged(app.id.clone()),
            icon: IconSource::Packaged(app.id.clone()),
        });
    }
    if let Some(link) = shortcut {
        let (link, name) = (link.to_path_buf(), app.name.clone());
        if let Some(p) = crate::sta::call(Duration::from_secs(15), move || super::pins::pinned_from_shortcut(&link, &name)).flatten() {
            return Some(p);
        }
    }
    let target = super::startapps::location(app).filter(|t| t.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && t.is_file())?;
    Some(PinnedApp {
        name: app.name.clone(),
        aumid: None,
        exe: Some(target.to_string_lossy().to_lowercase()),
        launch: Launch::Exe { path: target.clone(), args: String::new() },
        icon: IconSource::File(target),
    })
}

/// The dock app the pin file's own matching rules (`PinFile::find`: the AppUserModelID, the program, the program the pin starts) are
/// asked about, so that "already on the dock" means what it means for the dock's menu.
fn as_dock_app(p: &PinnedApp) -> DockApp {
    DockApp {
        key: p.aumid.clone().or_else(|| p.exe.clone()).unwrap_or_default(),
        name: p.name.clone(),
        pinned: false,
        launch: Some(p.launch.clone()),
        icon: p.icon.clone(),
        windows: Vec::new(),
        active: false,
    }
}

/// Whether the dock's own list has this app already.
pub fn already_on_dock(file: &PinFile, p: &PinnedApp) -> bool {
    // (an app with no identity at all cannot be told from another: it is not added)
    let app = as_dock_app(p);
    app.key.is_empty() || file.is_pinned(&app)
}

/// Adds the app to the end of the dock's list unless it is on it. `keep` is given the pin (the caller copies the shortcut into the
/// dock's own folder there, so that the pin does not depend on the Start menu's file) and returns what is stored. False: nothing changed.
pub fn add_to_dock(file: &mut PinFile, p: &PinnedApp, keep: impl FnOnce(PinnedApp) -> PinnedApp) -> bool {
    if already_on_dock(file, p) {
        return false;
    }
    file.pins.push(OwnPin::from_pinned(&keep(p.clone())));
    true
}

/// Copies a shortcut into `dir` under its own name, or under a free one (`Name (2).lnk`) when that name is taken: a copy never replaces
/// a file that is there (it may be another pin's own shortcut). `None` when it cannot be copied.
pub fn copy_link_unique(from: &Path, dir: &Path) -> Option<PathBuf> {
    let stem = from.file_stem()?.to_string_lossy().into_owned();
    let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "lnk".into());
    std::fs::create_dir_all(dir).ok()?;
    for n in 1..100 {
        let name = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} ({n}).{ext}") };
        let to = dir.join(name);
        if to.exists() {
            continue;
        }
        return std::fs::copy(from, &to).ok().map(|_| to);
    }
    None
}

/// The pin with its shortcut copied into `dir` (the dock's own folder), so that the dock's pin does not depend on a file the Start menu
/// owns. A pin that is not a shortcut, or whose copy fails, is returned as it was (it still works, it stays tied to the original).
pub fn localized(p: PinnedApp, dir: &Path) -> PinnedApp {
    match &p.launch {
        Launch::Link(original) => match copy_link_unique(original, dir) {
            Some(own) => PinnedApp { launch: Launch::Link(own), ..p },
            None => p,
        },
        _ => p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::model::IconSource;

    fn pin(id: &str) -> Pin {
        Pin { id: id.into(), name: id.into() }
    }

    fn ids(v: &[Pin]) -> Vec<&str> {
        v.iter().map(|p| p.id.as_str()).collect()
    }

    fn grid() -> Grid {
        // 6 columns of 100 x 90 cells at (28, 60), 3 rows, 14 tiles on this page, the page starting at the 19th shown pin
        Grid { origin: (28.0, 60.0), cell: (100.0, 90.0), cols: 6, rows: 3, first: 18, count: 14 }
    }

    #[test]
    fn a_press_is_a_click_until_it_has_moved_more_than_the_drag_distance() {
        let limit = (4.0, 4.0);
        assert!(!moved_enough(0.0, 0.0, limit));
        assert!(!moved_enough(4.0, -4.0, limit), "exactly the distance is still a click");
        assert!(moved_enough(4.5, 0.0, limit));
        assert!(moved_enough(0.0, -5.0, limit));
        assert!(moved_enough(-9.0, 9.0, limit));
        // the system's own distance is a few pixels, never 0
        let (x, y) = drag_distance();
        assert!(x >= 1 && y >= 1 && x < 100 && y < 100);
    }

    #[test]
    fn the_grid_says_which_slot_a_point_is_over_and_nothing_outside_it() {
        let g = grid();
        // the first cell and the cell next to it, then the second row
        assert_eq!(g.slot_at((30.0, 62.0)), Some(18));
        assert_eq!(g.slot_at((127.9, 62.0)), Some(18));
        assert_eq!(g.slot_at((128.1, 62.0)), Some(19));
        assert_eq!(g.slot_at((30.0, 151.0)), Some(24));
        // the last tile's own cell (14 tiles: row 2, column 1)
        assert_eq!(g.slot_at((140.0, 241.0)), Some(18 + 13));
        // the empty end of the last row means "last"
        assert_eq!(g.slot_at((600.0, 241.0)), Some(18 + 13));
        // outside the rectangle: above, left, right of the sixth column, below the third row
        assert_eq!(g.slot_at((27.9, 100.0)), None);
        assert_eq!(g.slot_at((100.0, 59.9)), None);
        assert_eq!(g.slot_at((28.0 + 600.0, 100.0)), None);
        assert_eq!(g.slot_at((100.0, 60.0 + 270.0)), None);
        // no tiles, no slots
        assert_eq!(Grid { count: 0, ..g }.slot_at((30.0, 62.0)), None);
        assert_eq!(g.cell_of(0), (0.0, 0.0));
        assert_eq!(g.cell_of(7), (1.0, 1.0));
    }

    #[test]
    fn where_the_pointer_is_comes_from_the_rectangles() {
        let scene = Scene { viewport: (642.0, 720.0), grid: Some(grid()), island: Some(Rect::new(800, 1300, 1700, 1380)) };
        // inside the panel: over a cell, or elsewhere in the panel
        assert_eq!(where_is((40.0, 70.0), (40, 70), &scene), Where::Grid(18));
        assert_eq!(where_is((40.0, 400.0), (40, 400), &scene), Where::Panel);
        // outside the panel: over the island's rectangle (screen px), or anywhere else
        assert_eq!(where_is((900.0, 600.0), (1000, 1340), &scene), Where::Island);
        assert_eq!(where_is((900.0, 600.0), (1000, 1200), &scene), Where::Outside);
        assert_eq!(where_is((-5.0, 100.0), (-5, 100), &scene), Where::Outside, "left of the window");
        assert_eq!(where_is((300.0, 720.0), (300, 720), &scene), Where::Outside, "the window's bottom edge is outside it");
        // the all-apps page has no grid
        let list = Scene { grid: None, ..scene };
        assert_eq!(where_is((40.0, 70.0), (40, 70), &list), Where::Panel);
        // a screen with no island known
        let none = Scene { island: None, ..scene };
        assert_eq!(where_is((900.0, 600.0), (1000, 1340), &none), Where::Outside);
        // the panel wins where the two overlap
        let over = Scene { island: Some(Rect::new(0, 0, 2000, 2000)), ..scene };
        assert_eq!(where_is((40.0, 400.0), (40, 400), &over), Where::Panel);
    }

    #[test]
    fn each_kind_of_item_may_be_dropped_only_where_it_means_something() {
        use super::Drop::*;
        use super::Item::*;
        // a pinned app: reordered in the grid, on the dock, and as a file out in the open when it has a shortcut
        assert_eq!(drop_for(Pinned, Where::Grid(5), Some(2), true), Reorder { to: 5 });
        assert_eq!(drop_for(Pinned, Where::Grid(2), Some(2), true), Nothing, "onto itself");
        assert_eq!(drop_for(Pinned, Where::Panel, Some(2), true), Nothing);
        assert_eq!(drop_for(Pinned, Where::Island, Some(2), false), PinToDock, "a packaged app can go to the dock");
        assert_eq!(drop_for(Pinned, Where::Outside, Some(2), true), HandToSystem);
        assert_eq!(drop_for(Pinned, Where::Outside, Some(2), false), NotAllowed, "a packaged app can go nowhere else");
        // an app of the list: no grid to reorder
        assert_eq!(drop_for(Listed, Where::Grid(0), None, true), Nothing);
        assert_eq!(drop_for(Listed, Where::Island, None, false), PinToDock);
        assert_eq!(drop_for(Listed, Where::Outside, None, true), HandToSystem);
        assert_eq!(drop_for(Listed, Where::Outside, None, false), NotAllowed);
        // a recommended file is a file drag wherever it is
        for at in [Where::Grid(1), Where::Panel, Where::Island, Where::Outside] {
            assert_eq!(drop_for(Recommended, at, None, false), HandToSystem);
        }
    }

    #[test]
    fn a_drop_the_system_ran_is_on_the_dock_when_it_ended_over_the_island() {
        let island = Some(Rect::new(800, 1300, 1700, 1380));
        assert!(dropped_on_island(Some((1000, 1340)), island));
        assert!(!dropped_on_island(Some((1000, 1200)), island));
        assert!(!dropped_on_island(Some((1700, 1340)), island), "the right edge is outside");
        assert!(!dropped_on_island(None, island), "a pointer that could not be read is nowhere");
        assert!(!dropped_on_island(Some((1000, 1340)), None), "no island known");
    }

    #[test]
    fn a_drop_the_system_ran_is_on_the_dock_only_when_the_island_itself_is_the_window_there() {
        assert!(is_island_window(Some(0x1234), Some(0x1234)));
        assert!(!is_island_window(Some(0x1234), Some(0x9999)), "another program's window is under the island's rectangle");
        assert!(!is_island_window(None, Some(0x1234)));
        assert!(!is_island_window(Some(0x1234), None), "the island's window is not known");
        assert!(!is_island_window(None, None));
    }

    #[test]
    fn the_feedback_follows_the_drop() {
        assert_eq!(feedback(Item::Pinned, Where::Island, Some(0), false), Feedback::DockTakes);
        assert_eq!(feedback(Item::Pinned, Where::Outside, Some(0), false), Feedback::NotAllowed);
        assert_eq!(feedback(Item::Pinned, Where::Outside, Some(0), true), Feedback::Plain);
        assert_eq!(feedback(Item::Pinned, Where::Grid(3), Some(0), true), Feedback::Plain);
        assert_eq!(feedback(Item::Listed, Where::Panel, None, false), Feedback::Plain);
    }

    #[test]
    fn dropping_a_pin_at_the_first_the_last_a_middle_place_or_on_itself() {
        // [a b c d e], take c (2)
        assert_eq!(move_to(5, 2, 0), [2, 0, 1, 3, 4], "first");
        assert_eq!(move_to(5, 2, 4), [0, 1, 3, 4, 2], "last");
        assert_eq!(move_to(5, 2, 3), [0, 1, 3, 2, 4], "between d and e");
        assert_eq!(move_to(5, 2, 1), [0, 2, 1, 3, 4], "between a and b");
        assert_eq!(move_to(5, 2, 2), [0, 1, 2, 3, 4], "onto itself");
        // beyond the end is the last place; a tile that is not there changes nothing
        assert_eq!(move_to(5, 0, 99), [1, 2, 3, 4, 0]);
        assert_eq!(move_to(5, 9, 1), [0, 1, 2, 3, 4]);
        assert!(move_to(0, 0, 0).is_empty());
        // one tile
        assert_eq!(move_to(1, 0, 3), [0]);
    }

    #[test]
    fn while_a_pin_is_held_the_others_stand_in_the_cells_that_make_room() {
        // not over the grid: everything where it is
        assert_eq!(cells_while_held(4, 1, None), [0, 1, 2, 3]);
        // tile 1 held over slot 3: tiles 2 and 3 shift one cell back, the held tile's landing cell is 3
        assert_eq!(cells_while_held(4, 1, Some(3)), [0, 3, 1, 2]);
        // tile 3 held over slot 0: the others shift forward
        assert_eq!(cells_while_held(4, 3, Some(0)), [1, 2, 3, 0]);
        assert_eq!(cells_while_held(4, 2, Some(2)), [0, 1, 2, 3], "over itself nothing moves");
    }

    #[test]
    fn a_pin_of_an_app_that_is_not_installed_keeps_its_place_in_the_file() {
        // file: a, GONE, b, c, GONE2, d; shown: a b c d at 0 2 3 5
        let file = [pin("a"), pin("GONE"), pin("b"), pin("c"), pin("GONE2"), pin("d")];
        let shown = shown_positions(&file, |p| !p.id.starts_with("GONE"));
        assert_eq!(shown, [0, 2, 3, 5]);
        // d (3rd shown) dropped first: d takes a's place, the others move up through the shown places; the missing ones stay put
        let perm = apply_move(file.len(), &shown, 3, 0);
        let out = permuted(&file, &perm);
        assert_eq!(ids(&out), ["d", "GONE", "a", "b", "GONE2", "c"]);
        // a dropped last
        let out = permuted(&file, &apply_move(file.len(), &shown, 0, 3));
        assert_eq!(ids(&out), ["b", "GONE", "c", "d", "GONE2", "a"]);
        // onto itself and an unknown pin: no change
        assert_eq!(permuted(&file, &apply_move(file.len(), &shown, 1, 1)), file.to_vec());
        assert_eq!(permuted(&file, &apply_move(file.len(), &shown, 9, 0)), file.to_vec());
        // nothing is lost or repeated, whatever is moved where
        for from in 0..4 {
            for to in 0..6 {
                let mut got: Vec<String> = permuted(&file, &apply_move(file.len(), &shown, from, to)).into_iter().map(|p| p.id).collect();
                got.sort();
                let mut want: Vec<String> = file.iter().map(|p| p.id.clone()).collect();
                want.sort();
                assert_eq!(got, want);
            }
        }
    }

    #[test]
    fn a_slot_of_the_second_page_is_a_global_index_and_a_position_on_the_page_by_subtraction() {
        // 40 pins, all shown; the page on the screen starts at the 19th (grid(): first 18, 14 tiles)
        let g = grid();
        let file: Vec<Pin> = (0..40).map(|i| pin(&format!("p{i}"))).collect();
        let shown = shown_positions(&file, |_| true);
        // the first cell of the page is the global slot 18, and a point past the last tile is the page's last slot (31)
        let to = g.slot_at((g.origin.0 + 5.0, g.origin.1 + 5.0)).unwrap();
        assert_eq!(to, 18);
        assert_eq!(local_slot(to, g.first, g.count), Some(0));
        assert_eq!(local_slot(31, g.first, g.count), Some(13));
        assert_eq!(local_slot(32, g.first, g.count), None, "the next page's first tile is not on this one");
        assert_eq!(local_slot(3, g.first, g.count), None, "an earlier page's tile is not on this one");
        assert_eq!(clamped_local_slot(3, g.first, g.count), 0);
        assert_eq!(clamped_local_slot(99, g.first, g.count), 13);
        assert_eq!(clamped_local_slot(20, g.first, 0), 0, "a page with no tiles is not a division by nothing");
        // the 21st pin (slot 20) dropped on the page's first cell takes the 19th's place; the file's order is the one the grid shows
        let out = permuted(&file, &apply_move(file.len(), &shown, 20, to));
        assert_eq!(out[18].id, "p20");
        assert_eq!(out[19].id, "p18");
        assert_eq!(out[20].id, "p19");
        assert_eq!(out[17].id, "p17");
        assert_eq!(out[21].id, "p21");
    }

    #[test]
    fn the_shown_pins_are_the_ones_the_grid_resolves_in_the_same_order() {
        let app = |id: &str| App { name: id.into(), id: id.into() };
        let apps = [app("A"), app("b"), app("C"), app("d")];
        // (ids differ in case from the apps' ids: a packaged app's id is written either way)
        let file = super::super::startpins::PinFile {
            version: 1,
            seeded: true,
            pins: vec![pin("D"), pin("GONE"), pin("B"), pin("a"), pin("GONE2"), pin("c")],
        };
        let shown = shown_in(&file, &apps);
        let resolved: Vec<&str> = file.resolve(&apps).into_iter().map(|(_, p)| p.id.as_str()).collect();
        let by_positions: Vec<&str> = shown.iter().map(|&i| file.pins[i].id.as_str()).collect();
        assert_eq!(by_positions, resolved);
        assert_eq!(shown, [0, 2, 3, 5]);
        // so dropping the tile in grid slot 3 ("c") on slot 0 changes the file the way the grid shows it
        let out = permuted(&file.pins, &apply_move(file.pins.len(), &shown, 3, 0));
        assert_eq!(ids(&out), ["c", "GONE", "D", "B", "GONE2", "a"]);
    }

    #[test]
    fn a_lost_pointer_needs_a_button_that_looks_up_for_several_looks_and_no_events_meanwhile() {
        let quiet = LostPointer::QUIET_MS;
        let mut l = LostPointer::default();
        // the button looks up but events keep coming (touch, pen, remote input): never lost
        for _ in 0..20 {
            assert!(!l.look(false, 50));
        }
        // the button down resets the count
        assert!(!l.look(false, quiet));
        assert!(!l.look(false, quiet));
        assert!(!l.look(true, quiet));
        assert!(!l.look(false, quiet));
        assert!(!l.look(false, quiet));
        // an event in between resets it too
        assert!(!l.look(false, 10));
        assert!(!l.look(false, quiet));
        assert!(!l.look(false, quiet));
        // quiet and up for LOOKS looks in a row: lost
        let mut l = LostPointer::default();
        for _ in 1..LostPointer::LOOKS {
            assert!(!l.look(false, quiet));
        }
        assert!(l.look(false, quiet));
    }

    #[test]
    fn the_click_that_ends_a_drag_does_nothing_and_the_next_one_works() {
        let mut g = ClickGate::default();
        assert!(g.allow(), "an ordinary click");
        g.drag_ended();
        assert!(!g.allow(), "the release of a drag");
        assert!(g.allow(), "the silence is used up by one click");
        // a drag ends, then a new press comes before any click was asked: the new press's click works
        g.drag_ended();
        g.pressed();
        assert!(g.allow());
    }

    #[test]
    fn something_that_is_not_a_permutation_changes_nothing() {
        let file = [pin("a"), pin("b"), pin("c")];
        assert_eq!(permuted(&file, &[0, 0, 1]), file.to_vec());
        assert_eq!(permuted(&file, &[0, 1]), file.to_vec());
        assert_eq!(permuted(&file, &[0, 1, 7]), file.to_vec());
        assert_eq!(ids(&permuted(&file, &[2, 0, 1])), ["c", "a", "b"]);
    }

    #[test]
    fn the_tiles_slide_as_the_docks_spring_and_a_new_target_starts_from_where_the_tile_is() {
        let at = |c: usize| (c as f32, 0.0);
        let mut s = Slides::at_rest(&[at(0), at(1), at(2)], SLIDE_MS);
        assert!(s.settled(0));
        assert_eq!(s.pos(1, 5), at(1));
        // tile 0 goes to cell 2, tile 2 to cell 0
        s.retarget(&[at(2), at(1), at(0)], 1000);
        assert!(!s.settled(1000));
        assert_eq!(s.pos(0, 1000), at(0), "it starts where it was");
        let mid = s.pos(0, 1000 + u64::from(SLIDE_MS) / 2);
        // (a spring: half the time is nine tenths of the way, and it never passes its cell)
        assert!(mid.0 > 1.7 && mid.0 < 2.0, "half the time is most of the way: {mid:?}");
        assert!((0..=SLIDE_MS).all(|t| (0.0..=2.0).contains(&s.pos(0, 1000 + u64::from(t)).0)));
        assert_eq!(s.pos(0, 1000 + u64::from(SLIDE_MS)), at(2));
        assert!(s.settled(1000 + u64::from(SLIDE_MS)));
        assert_eq!(s.pos(1, 1010), at(1), "a tile whose cell did not change does not move");
        // turned round in the middle: from where it is
        let mut t = Slides::at_rest(&[at(0)], SLIDE_MS);
        t.retarget(&[at(4)], 0);
        let here = t.pos(0, u64::from(SLIDE_MS) / 2);
        t.retarget(&[at(0)], u64::from(SLIDE_MS) / 2);
        assert_eq!(t.pos(0, u64::from(SLIDE_MS) / 2), here, "no jump when the target turns");
        assert_eq!(t.pos(0, u64::from(SLIDE_MS) * 2), at(0));
        // animations off: at once
        let mut off = Slides::at_rest(&[at(0)], 0);
        off.retarget(&[at(3)], 50);
        assert_eq!(off.pos(0, 50), at(3));
        assert!(off.settled(50));
        // a different number of tiles starts afresh
        let mut u = Slides::at_rest(&[at(0)], SLIDE_MS);
        u.retarget(&[at(1), at(2)], 7);
        assert_eq!(u.pos(1, 7), at(2));
    }

    fn listed(path: &str, shown: Option<&str>) -> Listed {
        Listed { path: PathBuf::from(path), shown: shown.map(str::to_string) }
    }

    #[test]
    fn a_shortcut_is_found_by_the_apps_name_without_regard_to_case_and_the_users_own_wins() {
        let files = vec![
            listed(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Paint.lnk", None),
            listed(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Accessories\Notepad.lnk", Some("Not Defteri")),
            listed(r"C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\PAINT.LNK", None),
            listed(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Işık Ayarları.lnk", None),
        ];
        let at = |i: usize| Some(files[i].path.clone());
        assert_eq!(pick_shortcut(&files, "paint"), at(2), "the later (the user's own) folder wins");
        assert_eq!(pick_shortcut(&files, "Notepad"), at(1), "by the file name");
        assert_eq!(pick_shortcut(&files, "Not Defteri"), at(1), "by the name Windows shows for it");
        assert_eq!(pick_shortcut(&files, "  not defteri "), at(1));
        assert_eq!(pick_shortcut(&files, "Işık Ayarları"), at(3));
        assert_eq!(pick_shortcut(&files, "Calculator"), None);
        assert_eq!(pick_shortcut(&files, ""), None);
        assert_eq!(pick_shortcut(&[], "Paint"), None);
    }

    #[test]
    fn the_names_a_folder_gives_its_shortcuts_are_read_from_its_desktop_ini() {
        let ini = "[.ShellClassInfo]\r\nLocalizedResourceName=@x.dll,-1\r\n\r\n[LocalizedFileNames]\r\nNotepad.lnk=@%SystemRoot%\\system32\\shell32.dll,-22042\r\nPaint.lnk=Boyama\r\nBroken.lnk=@nothing,-1\r\n=empty\r\nNoValue.lnk=\r\n\r\n[Other]\r\nX.lnk=Not this one\r\n";
        let resolve = |r: &str| (r == "@%SystemRoot%\\system32\\shell32.dll,-22042").then(|| "Not Defteri".to_string());
        let got = localized_names(ini, resolve);
        assert_eq!(got, vec![("Notepad.lnk".to_string(), "Not Defteri".to_string()), ("Paint.lnk".to_string(), "Boyama".to_string())]);
        assert!(localized_names("no sections at all", resolve).is_empty());
        // Windows writes the file in UTF-16 with a byte order mark
        let mut bytes = vec![0xFF, 0xFE];
        for u in "[LocalizedFileNames]\r\nA.lnk=Işık\r\n".encode_utf16() {
            bytes.extend(u.to_le_bytes());
        }
        assert_eq!(localized_names(&ini_text(&bytes), resolve), vec![("A.lnk".to_string(), "Işık".to_string())]);
        assert_eq!(ini_text(b"[LocalizedFileNames]"), "[LocalizedFileNames]");
    }

    #[test]
    fn a_resource_name_is_loaded_from_windows_own_files() {
        // a reference into Windows' own resources gives some text; a reference to nothing gives none
        assert!(resolve_resource("@%SystemRoot%\\system32\\shell32.dll,-22042").is_some());
        assert!(resolve_resource("@nothing-here.dll,-1").is_none());
    }

    #[test]
    fn a_packaged_app_has_no_shortcut_to_drag_and_a_classic_one_may() {
        let store = App { name: "Calculator".into(), id: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into() };
        assert_eq!(shortcut_of(&store), None, "no folder is even looked at for a package");
        let classic = App { name: "Zzz-no-such-app-1f3a9".into(), id: r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe".into() };
        assert_eq!(shortcut_of(&classic), None, "nothing in the Start menu has this name");
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-startdrag-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_drag_carries_a_copy_and_leaves_the_original_where_it_is() {
        let dir = tmp("copy");
        let original = dir.join("Paint.lnk");
        std::fs::write(&original, b"link bytes").unwrap();
        let root = dir.join("drag");
        let a = drag_copy(&original, &root).unwrap();
        let b = drag_copy(&original, &root).unwrap();
        assert_ne!(a.parent(), b.parent(), "a folder of its own for each drag");
        assert_eq!(a.file_name().unwrap(), "Paint.lnk");
        assert_eq!(std::fs::read(&a).unwrap(), b"link bytes");
        assert_eq!(std::fs::read(&original).unwrap(), b"link bytes");
        assert!(drag_copy(&dir.join("missing.lnk"), &root).is_err());
        // nothing is old enough to go yet; with a limit of nothing every folder goes, the original stays
        assert_eq!(sweep(&root, Duration::from_secs(3600)), 0);
        assert!(a.exists() && b.exists());
        assert_eq!(sweep(&root, Duration::ZERO), 2);
        assert!(!a.exists() && !b.exists() && original.exists());
        assert_eq!(sweep(&dir.join("nowhere"), Duration::ZERO), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn pinned(name: &str, aumid: Option<&str>, exe: Option<&str>, launch: Launch) -> PinnedApp {
        PinnedApp { name: name.into(), aumid: aumid.map(str::to_lowercase), exe: exe.map(str::to_lowercase), launch, icon: IconSource::File(PathBuf::from("x.exe")) }
    }

    #[test]
    fn an_app_already_on_the_dock_is_not_added_again_and_a_new_one_goes_to_the_end() {
        let chrome_from_taskbar = pinned("Chrome", None, Some(r"C:\a\chrome.exe"), Launch::Link(PathBuf::from(r"C:\cfg\dock-pins\Chrome.lnk")));
        let store = pinned("Hesap", Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), None, Launch::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        let mut file = PinFile::default();
        file.seed(&[chrome_from_taskbar, store]);
        // the same program through the Start menu's shortcut: found by the program
        let chrome_start = pinned("Google Chrome", None, Some(r"C:\a\Chrome.exe"), Launch::Link(PathBuf::from(r"C:\ProgramData\Start Menu\Programs\Google Chrome.lnk")));
        assert!(already_on_dock(&file, &chrome_start));
        assert!(!add_to_dock(&mut file, &chrome_start, |p| p));
        // the same package, other letter case
        let store_again = pinned("Calculator", Some("microsoft.windowscalculator_8wekyb3d8bbwe!app"), None, Launch::Packaged("microsoft.windowscalculator_8wekyb3d8bbwe!app".into()));
        assert!(!add_to_dock(&mut file, &store_again, |p| p));
        assert_eq!(file.pins.len(), 2);
        // something new goes to the end, through `keep`
        let paint = pinned("Paint", None, Some(r"C:\w\mspaint.exe"), Launch::Link(PathBuf::from(r"C:\ProgramData\Start Menu\Programs\Paint.lnk")));
        assert!(add_to_dock(&mut file, &paint, |mut p| {
            p.launch = Launch::Link(PathBuf::from(r"C:\cfg\dock-pins\Paint.lnk"));
            p
        }));
        assert_eq!(file.pins.len(), 3);
        assert_eq!(file.pins[2].name, "Paint");
        assert_eq!(file.pins[2].exe.as_deref(), Some(r"c:\w\mspaint.exe"));
        assert_eq!(file.pins[2].to_pinned().launch, Launch::Link(PathBuf::from(r"C:\cfg\dock-pins\Paint.lnk")), "the stored pin is what `keep` made");
        // and once it is there it is there
        assert!(!add_to_dock(&mut file, &paint, |p| p));
        assert_eq!(file.pins.len(), 3);
    }

    #[test]
    fn a_pin_with_no_identity_is_not_added() {
        let nameless = pinned("???", None, None, Launch::Exe { path: PathBuf::new(), args: String::new() });
        let mut file = PinFile::default();
        assert!(!add_to_dock(&mut file, &nameless, |p| p));
        assert!(file.pins.is_empty());
    }

    #[test]
    fn a_packaged_app_becomes_a_package_pin_without_asking_the_shell() {
        let app = App { name: "Hesap Makinesi".into(), id: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into() };
        let p = dock_pin_of(&app, None).unwrap();
        assert_eq!(p.aumid.as_deref(), Some("microsoft.windowscalculator_8wekyb3d8bbwe!app"));
        assert_eq!(p.launch, Launch::Packaged(app.id.clone()));
        assert_eq!(p.icon, IconSource::Packaged(app.id));
        assert!(p.exe.is_none());
    }

    #[test]
    fn a_copy_for_the_dock_never_takes_the_place_of_a_file_that_is_there() {
        let dir = tmp("unique");
        let from = dir.join("Start Paint.lnk");
        std::fs::write(&from, b"new").unwrap();
        let own = dir.join("dock-pins");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("Start Paint.lnk"), b"taskbar's own copy").unwrap();
        let got = copy_link_unique(&from, &own).unwrap();
        assert_eq!(got.file_name().unwrap(), "Start Paint (2).lnk");
        assert_eq!(std::fs::read(own.join("Start Paint.lnk")).unwrap(), b"taskbar's own copy", "the one that was there is untouched");
        assert_eq!(std::fs::read(&got).unwrap(), b"new");
        let again = copy_link_unique(&from, &own).unwrap();
        assert_eq!(again.file_name().unwrap(), "Start Paint (3).lnk");
        assert!(copy_link_unique(&dir.join("missing.lnk"), &own).is_none());
        // the pin follows its copy; a program pin, a package pin and a pin whose copy fails are left as they are
        let link_pin = pinned("Paint", None, Some("c:\\w\\mspaint.exe"), Launch::Link(from.clone()));
        let kept = localized(link_pin.clone(), &own);
        assert_eq!(kept.launch, Launch::Link(own.join("Start Paint (4).lnk")));
        assert_eq!(kept.exe, link_pin.exe);
        let exe_pin = pinned("Exe", None, Some("c:\\w\\x.exe"), Launch::Exe { path: PathBuf::from("c:\\w\\x.exe"), args: String::new() });
        assert_eq!(localized(exe_pin.clone(), &own), exe_pin);
        let gone = pinned("Gone", None, Some("c:\\w\\g.exe"), Launch::Link(dir.join("missing.lnk")));
        assert_eq!(localized(gone.clone(), &own), gone);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
