//! One tab's content: a location, its listing, selection, and the details view.
//!
//! Listing runs on a dedicated thread and streams batches back; the UI thread only
//! merges batches and paints. Navigating away cancels the in-flight listing.

use crate::t;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::entry::Filter;
use rf_core::{Entry, History, Location, SortDir, SortKey, SortSpec, format};

use crate::actions::*;
use crate::icons::{self, Category};
use crate::image_cache::ImageCache;
use crate::places::{self, Places};
use crate::theme::{self, Palette};

/// Details columns that are as wide as what they show.
mod auto_cols;
/// Columns for any Windows property (the "Diğer sütunlar…" columns).
pub(crate) mod prop_cols;
/// Sorting by tag, by path and by sync state.
pub(crate) mod sort_ext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Details,
    Grid,
    /// Finder-style Miller columns.
    Columns,
}

/// Look of the grid view (Files: Grid, Cards, List).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridStyle {
    #[default]
    Tiles,
    /// Wide tiles with type and size next to the picture.
    Cards,
    /// Small icon + name, many per row.
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    None,
    Type,
    Modified,
    Size,
    Name,
    Created,
    /// The modified date by calendar year / month / day (newest first).
    Year,
    Month,
    Day,
    /// The item's first tag (needs the tag store: computed on the UI thread only).
    Tag,
    /// The folder the item is in (search results).
    Folder,
    /// The value of a Windows property column (not in `ALL`: it is not remembered with a folder's layout, the
    /// property is not part of it). Needs the values read first, so the groups come from the property state.
    Property,
}

impl GroupBy {
    /// Stored in folder layouts and (by name) in the settings.
    pub const ALL: [(GroupBy, &'static str); 11] = [
        (GroupBy::None, "none"),
        (GroupBy::Name, "name"),
        (GroupBy::Type, "type"),
        (GroupBy::Modified, "modified"),
        (GroupBy::Size, "size"),
        (GroupBy::Created, "created"),
        (GroupBy::Year, "year"),
        (GroupBy::Month, "month"),
        (GroupBy::Day, "day"),
        (GroupBy::Tag, "tag"),
        (GroupBy::Folder, "folder"),
    ];

    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|(g, _)| *g == self).unwrap_or(0) as u8
    }

    pub fn from_index(i: u8) -> GroupBy {
        Self::ALL.get(i as usize).map_or(GroupBy::None, |(g, _)| *g)
    }

    /// Keys that need more than the entry itself (see [`view_from_sorted`]'s `extra`).
    fn needs_extra(self) -> bool {
        matches!(self, GroupBy::Tag | GroupBy::Folder | GroupBy::Property)
    }
}

const MONTHS: [&str; 12] = ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];

/// "Today / yesterday / this week ..." of a time.
fn relative_day(ms: i64, now: chrono::DateTime<chrono::Local>) -> (u32, String) {
    use chrono::{Datelike, TimeZone};
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single().filter(|_| ms > 0) else {
        return (6, "Bilinmiyor".into());
    };
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        d if d <= 0 => (0, "Bugün".into()),
        1 => (1, "Dün".into()),
        d if d < 7 && t.iso_week() == now.iso_week() => (2, "Bu hafta".into()),
        _ if t.year() == now.year() && t.month() == now.month() => (3, "Bu ay".into()),
        _ if t.year() == now.year() => (4, "Bu yıl".into()),
        _ => (5, "Daha eski".into()),
    }
}

/// The calendar year, month or day of a time; the order puts the newest first.
fn calendar_key(g: GroupBy, ms: i64) -> (u32, String) {
    use chrono::{Datelike, TimeZone};
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single().filter(|_| ms > 0) else {
        return (u32::MAX, "Bilinmiyor".into());
    };
    let (y, m, d) = (t.year().clamp(0, 9999) as u32, t.month(), t.day());
    match g {
        GroupBy::Year => (10_000 - y, y.to_string()),
        GroupBy::Month => ((10_000 - y) * 12 + (12 - m), format!("{} {y}", crate::i18n::t(MONTHS[(m as usize - 1).min(11)]))),
        _ => ((10_000 - y) * 372 + (12 - m) * 31 + (31 - d), format::datetime_full(ms).split(' ').next().unwrap_or_default().to_string()),
    }
}

/// A name split where an ellipsis may cut it: the part before the extension and the extension (with its dot), which stays whole.
/// A folder, a name without an extension, a name that is only an extension, a long "extension" (a dot in a sentence) and a
/// right-to-left name are not split. Pure.
fn split_for_ellipsis(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir || crate::rtl::has_rtl(name) {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 && !name[i..].contains(' ') => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// What every tile of one frame of the grid shares (see `FolderView::grid_cell`).
struct GridCellCtx {
    hover_select: bool,
    p: Palette,
    tile_w: Pixels,
    row_h: Pixels,
    thumb_px: u32,
    style: GridStyle,
    visual_px: Pixels,
    show_ext: bool,
    renamed: Option<(Arc<str>, Arc<str>)>,
}

/// One line of the GROUPED Grid (issue 130: "grouping is only drawn in the Details view"). The list draws lines of one height,
/// so the grouped Grid is made of slices one group header high: a header is one slice, a row of tiles spans `grid_slices().0`
/// of them, each slice showing its part of the row.
#[derive(Clone, Debug, PartialEq)]
enum GridUnit {
    Header(SharedString, usize),
    /// Part `part` of the row of tiles `start..start + len` (visible indices).
    Tiles { start: usize, len: usize, part: usize },
}

/// The grouped Grid's lines from the grouped rows: each group's items in rows of `cols` tiles of `slices` slices each. Also, for
/// every visible index, the line its row of tiles starts at.
/// [`FolderView::cell_hover`]'s count: the tile `vi` and over how many of its slices the pointer is told to be.
fn grid_hover_step(now: Option<(usize, u32)>, vi: usize, on: bool) -> Option<(usize, u32)> {
    match (on, now) {
        (true, Some((h, n))) if h == vi => Some((vi, n + 1)),
        (true, _) => Some((vi, 1)),
        (false, Some((h, n))) if h == vi => (n > 1).then(|| (vi, n - 1)),
        (false, other) => other,
    }
}

fn grid_units(rows: &[Row], cols: usize, slices: usize) -> (Vec<GridUnit>, Vec<usize>) {
    let (cols, slices) = (cols.max(1), slices.max(1));
    let (mut units, mut first_of_vi) = (Vec::new(), Vec::new());
    let flush = |units: &mut Vec<GridUnit>, first_of_vi: &mut Vec<usize>, start: usize, end: usize| {
        let mut s = start;
        while s < end {
            let len = cols.min(end - s);
            let first = units.len();
            first_of_vi.extend(std::iter::repeat_n(first, len));
            units.extend((0..slices).map(|part| GridUnit::Tiles { start: s, len, part }));
            s += len;
        }
    };
    let mut run: Option<(usize, usize)> = None;
    for r in rows {
        match r {
            Row::Header(label, n) => {
                if let Some((a, b)) = run.take() {
                    flush(&mut units, &mut first_of_vi, a, b);
                }
                units.push(GridUnit::Header(label.clone(), *n));
            }
            Row::Item(vi) => {
                run = Some(match run {
                    Some((a, _)) => (a, vi + 1),
                    None => (*vi, vi + 1),
                });
            }
        }
    }
    if let Some((a, b)) = run {
        flush(&mut units, &mut first_of_vi, a, b);
    }
    (units, first_of_vi)
}

/// A line of the details list when grouping is on.
#[derive(Clone)]
enum Row {
    Header(SharedString, usize),
    Item(usize),
}

fn group_key(g: GroupBy, e: &Entry, now: chrono::DateTime<chrono::Local>) -> (u32, String) {
    match g {
        GroupBy::None | GroupBy::Tag | GroupBy::Folder | GroupBy::Property => (0, String::new()),
        GroupBy::Created => relative_day(e.created_ms, now),
        GroupBy::Year | GroupBy::Month | GroupBy::Day => calendar_key(g, e.modified_ms),
        GroupBy::Type => {
            if e.is_dir() { (0, "Klasörler".into()) } else { (1, icons::type_label(e)) }
        }
        GroupBy::Modified => relative_day(e.modified_ms, now),
        GroupBy::Size => {
            if e.is_dir() {
                return (0, "Klasörler".into());
            }
            match e.size {
                0 => (1, "Boş".into()),
                s if s < 16 * 1024 => (2, "Çok küçük (< 16 KB)".into()),
                s if s < 1024 * 1024 => (3, "Küçük (< 1 MB)".into()),
                s if s < 128 * 1024 * 1024 => (4, "Orta (< 128 MB)".into()),
                s if s < 1024 * 1024 * 1024 => (5, "Büyük (< 1 GB)".into()),
                _ => (6, "Çok büyük (≥ 1 GB)".into()),
            }
        }
        GroupBy::Name => {
            let c = e.name.chars().next().unwrap_or('#');
            if c.is_alphabetic() {
                let up: String = if c == 'i' { "İ".into() } else { c.to_uppercase().collect() };
                (1, up)
            } else {
                (0, "#".into())
            }
        }
    }
}

struct Marquee {
    start: Point<Pixels>,
    cur: Point<Pixels>,
    base: HashSet<u32>,
}

pub enum FolderEvent {
    OpenInNewTab(Location),
    /// Open the workspace's sidebar context menu for a Home drive card.
    SidebarMenu(PathBuf),
    BulkRename(Vec<PathBuf>),
    AddToShelf(Vec<PathBuf>),
    /// Dual pane: copy/move the items into the other pane's folder.
    ToOtherPane(rf_win::fileops::Transfer, Vec<PathBuf>),
    /// Dual pane: show a folder in the other pane.
    OpenInOtherPane(Location),
    OpenInNewWindow(Location),
    /// Space / Esc: open or close Quick Look for the focused item.
    QuickLook,
    /// Browsing into a protected archive: (archive, the given password was wrong).
    NeedPassword(PathBuf, bool),
    /// Open/Save dialog: files were opened (double click, Enter): they are the answer.
    Picked(Vec<PathBuf>),
    /// Settings → "Typing filters this folder": a letter typed in the list opens the filter bar with it (issue 106).
    StartFilter(String),
}

/// Details columns shown right now (see `FolderView::visible_cols`).
#[derive(Clone, Copy)]
struct Cols {
    modified: bool,
    created: bool,
    kind: bool,
    size: bool,
    tags: bool,
    git: bool,
    sync: bool,
    /// commit message, author, date, hash
    commit: [bool; 4],
}

/// The Name column's index for the column drag (the other columns are 0..11, see `col_widths`).
const NAME_COL: usize = 11;

/// Drag payload of a details-header column edge.
#[derive(Clone)]
struct ColumnResize;
struct ColumnGhost;
impl Render for ColumnGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Quick Look is showing (Esc closes it instead of clearing the selection).
pub static QUICK_LOOK_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Filter + sort + grouping result, computable on any thread.
struct View {
    visible: Vec<u32>,
    /// Every entry in sort order (filter applied after): a filter change reuses it, no sort.
    sorted: Arc<Vec<u32>>,
    rows: Vec<Row>,
    row_of_vi: Vec<usize>,
    /// What it was computed with (discarded if the user changed any of these meanwhile).
    spec: (Filter, SortSpec, GroupBy),
    len: usize,
}

fn compute_view(entries: &[Entry], filter: &Filter, sort: SortSpec, group: GroupBy, extra: Extra) -> View {
    compute_view_ranked(entries, filter, sort, group, extra, None)
}

/// [`compute_view`] for the sort keys that sort by a rank made beforehand (tag, path: see
/// `sort_ext`). Without the ranks the view is sorted by name and says so in its `spec` (it is
/// then not taken for the view of the wanted sort).
fn compute_view_ranked(entries: &[Entry], filter: &Filter, sort: SortSpec, group: GroupBy, extra: Extra, ranks: Option<&[u32]>) -> View {
    let mut all: Vec<u32> = (0..entries.len() as u32).collect();
    let sort = if sort.key.needs_ranks() && ranks.is_none() { SortSpec { key: SortKey::Name, ..sort } } else { sort };
    match ranks {
        Some(r) => sort.sort_indices_ranked(entries, r, &mut all),
        None => sort.sort_indices(entries, &mut all),
    }
    view_from_sorted(entries, Arc::new(all), filter, sort, group, extra)
}

/// The group of an entry (by its index) for the keys the entry alone can't give: its tag, its
/// folder. Without it such a grouping puts everything in one nameless group.
type Extra<'a> = Option<&'a dyn Fn(u32) -> (u32, String)>;

/// [`compute_view`] from entries already in sort order: filtering only (typing in the filter
/// box of a 100,000-file folder costs a pass over the names, not a sort per keystroke).
fn view_from_sorted(entries: &[Entry], sorted: Arc<Vec<u32>>, filter: &Filter, sort: SortSpec, group: GroupBy, extra: Extra) -> View {
    let mut v: Vec<u32> = sorted.iter().copied().filter(|&i| filter.accepts(&entries[i as usize])).collect();
    let mut rows = Vec::new();
    let mut row_of_vi = Vec::new();
    if group != GroupBy::None {
        let now = chrono::Local::now();
        let keys: std::collections::HashMap<u32, (u32, String)> = v
            .iter()
            .map(|&i| match extra {
                Some(f) if group.needs_extra() => (i, f(i)),
                _ => (i, group_key(group, &entries[i as usize], now)),
            })
            .collect();
        // stable: keeps the sort order inside each group
        v.sort_by(|a, b| keys[a].cmp(&keys[b]));
        let mut current: Option<&(u32, String)> = None;
        let mut header_ix = 0;
        row_of_vi = Vec::with_capacity(v.len());
        for (vi, i) in v.iter().enumerate() {
            let k = &keys[i];
            if current != Some(k) {
                current = Some(k);
                header_ix = rows.len();
                rows.push(Row::Header(k.1.clone().into(), 0));
            }
            if let Row::Header(_, n) = &mut rows[header_ix] {
                *n += 1;
            }
            row_of_vi.push(rows.len());
            rows.push(Row::Item(vi));
        }
    }
    View { visible: v, sorted, rows, row_of_vi, spec: (filter.clone(), sort, group), len: entries.len() }
}

enum Msg {
    Batch(Vec<Entry>),
    /// Final sorted view computed on the listing thread (keeps big sorts off the UI thread).
    View(View),
    /// Search hits: entries plus their parent folders.
    Hits(Vec<Entry>, Vec<Arc<std::path::Path>>),
    /// Shell folder items (a phone, Network…): entries plus what the shell knows about each.
    Shell(Vec<Entry>, Vec<rf_win::shellfolder::ShellChild>),
    /// The shell's own name for the folder being listed.
    ShellName(String),
    /// What a search is doing now, shown while it runs (the owner, issue 67: "bu adımları ui'da gösterelim de kullanıcıya. sonra
    /// yavaş demesinler").
    Stage(SearchStage),
    Done(Result<usize, String>),
}

/// The step a search is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchStage {
    /// rustyfiles' own name index.
    Index,
    /// Windows Search (names and contents) is asked.
    Windows,
    /// Windows Search did not answer: the disk is walked instead.
    WindowsSilent,
    /// The disk is walked (no index covers the folder).
    Walk,
}

impl SearchStage {
    pub(crate) fn text(self) -> &'static str {
        match self {
            SearchStage::Index => crate::t!("Dizinde aranıyor…"),
            SearchStage::Windows => crate::t!("Windows Search'e soruluyor…"),
            SearchStage::WindowsSilent => crate::t!("Windows Search cevap vermedi, disk taranıyor…"),
            SearchStage::Walk => crate::t!("Disk taranıyor…"),
        }
    }
}

/// A folder's size in the Size column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirSize {
    /// Being measured: bytes counted so far.
    Partial(u64),
    Done(rf_win::foldersize::FolderSize),
}

impl From<rf_win::foldersize::SizeState> for DirSize {
    fn from(s: rf_win::foldersize::SizeState) -> Self {
        match s {
            rf_win::foldersize::SizeState::Partial(b) => DirSize::Partial(b),
            // a size remembered from the last run stands in until the new measurement ends; the
            // fresh value then replaces it without any marking
            rf_win::foldersize::SizeState::Provisional(s) | rf_win::foldersize::SizeState::Done(s) => DirSize::Done(s),
        }
    }
}

impl DirSize {
    /// "…" until something is counted, then the running total with "…", then the size.
    pub(crate) fn text(self, on_disk: bool) -> String {
        match self {
            DirSize::Partial(0) => "…".into(),
            DirSize::Partial(b) => format!("{}…", format::size(b)),
            DirSize::Done(s) => format::size(Self::bytes(s, on_disk)),
        }
    }

    /// The size to show and sort by; without a known cluster size the logical size stands in.
    fn bytes(s: rf_win::foldersize::FolderSize, on_disk: bool) -> u64 {
        if on_disk { s.allocated.unwrap_or(s.bytes) } else { s.bytes }
    }
}

pub struct FolderView {
    pub history: History,
    /// Set by the workspace while this view is one of two panes.
    pub(crate) dual: bool,
    /// Set by the workspace: this is the pane that owns the settings page's glass lens (the focused pane, when it shows Settings). The
    /// other pane of a split tab that shows Settings too keeps the flat selection fill, so two panes never fight over one lens slot.
    pub(crate) lens_owner: bool,
    /// (library file, default save folder) of the library being shown (paste/new item target).
    pub(crate) library_save: Option<(PathBuf, PathBuf)>,
    places: Entity<Places>,
    pub(crate) images: Entity<ImageCache>,
    pub view_mode: ViewMode,
    grid_cols: usize,
    /// Rows per column of the List filled column by column (issue 170), as drawn last; 0 when the folder is not shown so.
    list_rows: usize,
    /// The last frame showed an unfinished download's file (`.hdpart`) with its progress: the next one is drawn soon after.
    parts_shown: std::cell::Cell<bool>,
    part_tick: bool,
    /// The List filled column by column: each column's left edge and width as drawn last (content coordinates).
    list_cols: Vec<(f32, f32)>,
    /// The column widths measured from the names, and what they were measured for (see `list_column_widths`).
    list_widths: Option<(u64, Vec<f32>)>,
    /// Index into GRID_SIZES.
    grid_size: u8,
    /// Index into ROW_SIZES (Details row height), the "Size" slider in Details.
    row_size: u8,
    /// Column resize in progress: (column, mouse x at start, width at start), live widths.
    col_drag: Option<(usize, f32, f32)>,
    col_live: Option<[f32; 11]>,
    /// The grouped Grid's lines and, per visible index, the line its row of tiles starts at; with what they were made for (rows,
    /// visible items, columns, slices per row).
    grid_units: std::rc::Rc<(Vec<GridUnit>, Vec<usize>)>,
    grid_units_for: (usize, usize, usize, usize),
    /// The Name column's width while a divider is dragged (`NAME_COL`), before it is saved.
    name_live: Option<f32>,
    /// At the start of a column drag: the Name column's width and the room left over at the right end.
    drag_room: (f32, f32),
    /// The columns dragged since the last save, one bit each: only these get a stored width (the others stay automatic).
    col_dragged: u16,
    col_save_gen: u64,
    /// What the automatic columns measure in this listing, and what that measurement was made for (`auto_cols.rs`).
    auto_cols: auto_cols::AutoCols,
    auto_key: Option<auto_cols::Key>,
    pub(crate) grid_style: GridStyle,
    content_width: Pixels,
    pub(crate) entries: Vec<Entry>,
    /// Search mode only: parent folder of each entry (aligned with `entries`).
    parents: Vec<Arc<std::path::Path>>,
    /// Shell folders only: what the shell knows about each entry (aligned with `entries`).
    pub(crate) shell_items: Vec<rf_win::shellfolder::ShellChild>,
    /// Shell folders only: the shell's name for the folder (when the location carries none).
    shell_title: Option<String>,
    /// The step of the search under way (`Msg::Stage`), cleared when it is done.
    pub(crate) search_stage: Option<SearchStage>,
    visible: Vec<u32>,
    /// The entries in sort order, for the sort and the entries generation it was made with.
    sorted_cache: Option<(SortSpec, u64, Arc<Vec<u32>>)>,
    /// Changes whenever `entries` do (cache key of `sorted_cache`).
    entries_gen: u64,
    /// Whether any entry is a cloud file, for the entries generation it was worked out for (the
    /// Sync column; asked several times per frame).
    cloud_cache: std::cell::Cell<Option<(u64, bool)>>,
    pub sort: SortSpec,
    pub group: GroupBy,
    /// Labels of the groups folded away (their header stays; a click on it toggles).
    collapsed: HashSet<String>,
    /// Tagged paths → (order, name) of their first tag, as of the last grouping by tag.
    tag_snap: Arc<std::collections::HashMap<PathBuf, (u32, String)>>,
    /// Details rows (headers + items) when grouping; empty = identity with `visible`.
    rows: Vec<Row>,
    /// visible index → row index (grouping only).
    row_of_vi: Vec<usize>,
    pub filter: Filter,
    pub(crate) selected: HashSet<u32>,
    /// Columns for Windows properties: nothing is allocated or started until one is chosen.
    props: prop_cols::PropState,
    /// Ranks for a sort by tag or path (see `sort_ext`); empty and idle for every other key.
    sortx: sort_ext::SortxState,
    cursor: Option<usize>,
    anchor: Option<usize>,
    pub loading: bool,
    pub error: Option<String>,
    pub load_ms: Option<u128>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    /// Bumped on every navigation; a late watcher for an old folder is discarded.
    watch_gen: u64,
    load_started: Instant,
    _load_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    // Inactive tabs leave the element tree, but their settings scroll position lives with the pane.
    settings_scroll: ScrollHandle,
    pub focus: FocusHandle,
    /// Re-select this name after the next load (e.g. after going up a level).
    reselect: Option<String>,
    /// A rename that is under way: the item's name on disk until now, and the name it is getting. Its row shows the NEW name from
    /// the moment the rename box closes. WHY (the user: "isim değiştikten sonra çok kısa eski ismi gösterip sonra yeni isim oluyor
    /// ya, o olmasın"): the rename itself is done off this thread and the list learns of it from the folder's watcher, so for a
    /// moment the row had nothing but the entry's old name to show. Taken away when the listing no longer holds the old name
    /// (the rename landed) or the rename did not happen.
    renamed: Option<(Arc<str>, Arc<str>)>,
    /// Start inline rename on `reselect` once it appears (new folder flow).
    rename_after_load: bool,
    /// Show the item menu on `reselect` once it is selected, in this window (a right-click in another column of the columns
    /// layout, issue 183).
    menu_after_load: Option<AnyWindowHandle>,
    /// Items that just arrived (a paste): selected as they show up in the list, until all are
    /// there or the deadline passes (the watcher can lag behind the copy).
    reselect_many: Option<(Vec<String>, Instant)>,
    /// Begin inline rename on the next render (needs a Window).
    pending_rename: bool,
    /// Shown by the Open/Save dialog: opening a file picks it instead.
    picking: bool,
    /// A recent item of the Home page pressed with the left button, and where: moved far enough, it is dragged out (`drag_paths_out`).
    home_press: Option<(PathBuf, Point<Pixels>)>,
    /// Ctrl and the wheel turned so far toward the next size step (a touchpad turns in small pixel steps).
    zoom_acc: f32,
    /// The mouse went down on an item's name (set by the name label, read by the row).
    name_hit: bool,
    /// Explorer's "click the name of the selected item again": (entry, serial) armed until the
    /// double-click time passes without a second click or a drag.
    slow_click: Option<(u32, u64)>,
    slow_click_serial: u64,
    /// Settings → "Select on hover": the entry under the pointer and its serial, selected once the pointer has rested on it.
    hover_pick: Option<(u32, u64)>,
    /// The grouped Grid's tile under the pointer, and over how many of its slices (`grid_slices`: a row is drawn as several slices, each
    /// a clipped copy of the whole row, so the pointer is over one copy of a tile at a time and a hover style lit only that slice's
    /// strip of it, issue 157). The tile's fill follows this instead.
    grid_hover: Option<(usize, u32)>,
    /// Serial of the last click on a file in a column right of the list's (`pick_in_column`); a later click cancels a pending one.
    static_pick: u64,
    /// What the first click of a possible double click was made on (`completes_double`).
    first_click: Option<FirstClick>,
    /// Counts the listings shown (every `load`): a click belongs to the listing it was made in.
    load_serial: u64,
    /// Entries of an in-flight background refresh (swapped in atomically when complete).
    soft_pending: Option<Vec<Entry>>,
    /// When Space opened Quick Look (a long press closes it again on release).
    space_hold: Option<Instant>,
    /// When the current soft reload (watcher) started (for the live trace).
    soft_started: Instant,
    _watch: Option<rf_win::watch::DirWatch>,
    _watch_task: Option<Task<()>>,
    pub renaming: Option<(u32, Entity<gpui_kit::component::input::InputState>)>,
    recycle: Option<Entity<crate::recycle_view::RecycleView>>,
    /// The disk usage page of this tab (kept while the tab stays on that folder).
    disk_usage: Option<Entity<crate::disk_usage::DiskUsageView>>,
    /// HyperDownload's page, while this view shows it.
    downloads: Option<Entity<crate::downloads_view::DownloadsView>>,
    /// Git status of the repository containing this folder (if any).
    pub git: Option<Arc<rf_win::git::GitStatus>>,
    /// Last commit per child of the current folder (commit columns), keyed by name.
    git_commits: Option<Arc<std::collections::HashMap<String, rf_win::git::Commit>>>,
    /// Columns view: listings of ancestor / next folders (sorted, filtered).
    column_cache: std::collections::HashMap<PathBuf, Arc<Vec<Entry>>>,
    column_pending: HashSet<PathBuf>,
    /// Folder sizes for the Size column: clicked ("Boyutu hesapla") or all visible ones
    /// (Settings → "Calculate folder sizes").
    folder_sizes: std::collections::HashMap<PathBuf, DirSize>,
    /// Settings → Layout → "Adaptive": decide details/grid once this folder's listing is in.
    adaptive_pending: bool,
    /// The view's first folder has been given its layout (with "sync layout" on, only that first one takes the shared layout).
    layout_seeded: bool,
    /// Requested this frame; started together as one batch.
    size_queue: Vec<PathBuf>,
    size_batches: Vec<(Arc<rf_win::foldersize::SizeBatch>, Vec<PathBuf>)>,
    size_polling: bool,
    col_scroll: ScrollHandle,
    col_key: Option<(PathBuf, Option<PathBuf>)>,
    /// Per-ancestor-column scroll handles, and which (dir, highlighted child) we already scrolled to.
    col_scrolls: std::collections::HashMap<PathBuf, UniformListScrollHandle>,
    col_scrolled: HashSet<(PathBuf, String)>,
    _git_task: Option<Task<()>>,
    /// The overlays other programs put on the current folder's icons (Google Drive, TortoiseGit, ...; `rf_win::overlays`): item name →
    /// picture index. Settings > "show_overlays".
    overlays: Option<Arc<std::collections::HashMap<Arc<str>, i32>>>,
    /// Set to stop the questions of a folder left (a slow handler's folder must not hold up the next one).
    overlay_cancel: Arc<AtomicBool>,
    _overlay_task: Option<Task<()>>,
    overlay_soon: Option<Task<()>>,
    /// When git status was last asked for, and a later ask waiting (changes coming in fast).
    git_last: Option<Instant>,
    git_soon: Option<Task<()>>,
    /// Re-renders once a load has been running for a while (shows the "loading" placeholder).
    _slow_task: Option<Task<()>>,
    /// Where a left-button press started on a row (for drag-out detection).
    drag_origin: Option<RowPress>,
    /// Rubber-band selection in list-content coordinates.
    marquee: Option<Marquee>,
    /// Type-to-select buffer and when it was last extended.
    typeahead: (String, Instant),
    _rename_sub: Option<Subscription>,
}

impl EventEmitter<FolderEvent> for FolderView {}

impl Focusable for FolderView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FolderView {
    pub fn new(location: Location, places: Entity<Places>, images: Entity<ImageCache>, cx: &mut Context<Self>) -> Self {
        cx.observe(&places, |_, _, cx| cx.notify()).detach();
        cx.observe(&images, |_, _, cx| cx.notify()).detach();
        // a tag given or taken: the groups by tag follow
        cx.observe_global::<crate::tags::Tags>(|this, cx| {
            // (a sort by tag makes its ranks again; any other key: one comparison)
            this.sortx_tags_changed(cx);
            if this.group == GroupBy::Tag {
                this.refresh_tag_snap(cx);
                this.rebuild_visible();
            }
            cx.notify();
        })
        .detach();
        cx.observe_global::<crate::settings::Settings>(|this, cx| {
            let s = cx.global::<crate::settings::Settings>();
            let (hidden, system, dot) = (s.show_hidden, s.show_system_files, !s.show_dot_files);
            if (this.filter.show_hidden, this.filter.show_system, this.filter.dot_hidden) != (hidden, system, dot) {
                this.filter.show_hidden = hidden;
                this.filter.show_system = system;
                this.filter.dot_hidden = dot;
                // (the side columns were listed with the old rules)
                this.column_cache.clear();
                this.rebuild_visible();
            }
            // folders show (and sort by) size on disk or the sum of their files, as set
            this.resort_for_sizes(cx);
            // turning a commit column on loads the commits
            if this.git.is_some() && this.git_commits.is_none() && Self::commit_cols_on(cx) {
                this.refresh_git(cx);
            }
            cx.notify();
        })
        .detach();
        let prefs = cx.global::<crate::settings::Settings>().clone();
        let mut this = Self {
            history: History::new(location),
            dual: false,
            lens_owner: false,
            library_save: None,
            places,
            images,
            view_mode: match prefs.default_view {
                // Adaptive starts in details and switches once the listing shows a media folder
                crate::settings::DefaultView::Details | crate::settings::DefaultView::Adaptive => ViewMode::Details,
                crate::settings::DefaultView::Grid => ViewMode::Grid,
                crate::settings::DefaultView::Columns => ViewMode::Columns,
            },
            grid_cols: 1,
            list_rows: 0,
            parts_shown: std::cell::Cell::new(false),
            part_tick: false,
            list_cols: Vec::new(),
            list_widths: None,
            grid_size: DEFAULT_GRID_SIZE,
            row_size: DEFAULT_ROW_SIZE,
            col_drag: None,
            grid_units: Default::default(),
            grid_units_for: (0, 0, 0, 0),
            name_live: None,
            drag_room: (0., 0.),
            col_live: None,
            col_dragged: 0,
            col_save_gen: 0,
            auto_cols: Default::default(),
            auto_key: None,
            grid_style: GridStyle::Tiles,
            content_width: px(900.),
            entries: Vec::new(),
            sorted_cache: None,
            entries_gen: 0,
            cloud_cache: Default::default(),
            parents: Vec::new(),
            shell_items: Vec::new(),
            shell_title: None,
            search_stage: None,
            visible: Vec::new(),
            sort: SortSpec {
                key: prefs.default_sort,
                dir: if prefs.default_sort_desc { SortDir::Desc } else { SortDir::Asc },
                dirs_first: prefs.folders_first,
                files_first: !prefs.folders_first && prefs.files_first,
            },
            group: GroupBy::None,
            collapsed: HashSet::new(),
            tag_snap: Default::default(),
            rows: Vec::new(),
            row_of_vi: Vec::new(),
            filter: Filter {
                show_hidden: prefs.show_hidden,
                show_system: prefs.show_system_files,
                dot_hidden: !prefs.show_dot_files,
                ..Filter::default()
            },
            selected: HashSet::new(),
            props: prop_cols::PropState::default(),
            sortx: Default::default(),
            cursor: None,
            anchor: None,
            loading: false,
            error: None,
            load_ms: None,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            watch_gen: 0,
            load_started: Instant::now(),
            _load_task: None,
            scroll: UniformListScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            reselect: None,
            menu_after_load: None,
            renamed: None,
            rename_after_load: false,
            reselect_many: None,
            pending_rename: false,
            picking: false,
            home_press: None,
            zoom_acc: 0.0,
            name_hit: false,
            slow_click: None,
            slow_click_serial: 0,
            hover_pick: None,
            grid_hover: None,
            static_pick: 0,
            first_click: None,
            load_serial: 0,
            soft_pending: None,
            space_hold: None,
            soft_started: Instant::now(),
            _watch: None,
            _watch_task: None,
            renaming: None,
            _rename_sub: None,
            drag_origin: None,
            marquee: None,
            typeahead: (String::new(), Instant::now()),
            recycle: None,
            disk_usage: None,
            downloads: None,
            git: None,
            git_commits: None,
            column_cache: Default::default(),
            column_pending: HashSet::new(),
            folder_sizes: Default::default(),
            adaptive_pending: false,
            layout_seeded: false,
            size_queue: Default::default(),
            size_batches: Default::default(),
            size_polling: false,
            col_scroll: ScrollHandle::new(),
            col_key: None,
            col_scrolls: Default::default(),
            col_scrolled: HashSet::new(),
            _git_task: None,
            overlays: None,
            overlay_cancel: Arc::new(AtomicBool::new(false)),
            _overlay_task: None,
            overlay_soon: None,
            git_last: None,
            git_soon: None,
            _slow_task: None,
        };
        this.load(cx);
        this
    }

    /// "Bağlantıdan indir…": asks for a link (one on the clipboard is filled in) and downloads it into this folder with HyperDownload.
    pub fn download_from_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Location::Path(dir) = self.location().clone() else { return };
        crate::hyperdownload::ask_link(dir, window, cx);
    }

    pub fn location(&self) -> &Location {
        self.history.current()
    }

    pub fn title(&self, cx: &App) -> String {
        if let Location::Tag(id) = self.location() {
            if let Some(d) = cx.global::<crate::tags::Tags>().0.def(*id) {
                return format!("{} {}", crate::t!("Etiket:"), crate::i18n::td(&d.name));
            }
        }
        if let Location::Shell { name, .. } = self.location() {
            if name.is_empty() {
                if let Some(t) = &self.shell_title {
                    return t.clone();
                }
            }
        }
        if let Location::Path(p) = self.location() {
            // (the cache only: reading the folder's attributes here would hold the window on a place that has to wake up first;
            // `warm_labels` fills the cache from a worker)
            if let Some(l) = rf_win::localized::label_if_known(p) {
                return l.to_string();
            }
        }
        if let Location::DiskUsage(p) = self.location() {
            return crate::disk_usage::title(p);
        }
        crate::i18n::td(&self.location().title())
    }

    pub fn tab_icon(&self) -> L {
        match self.location() {
            Location::Home => L::House,
            Location::Settings => L::Settings,
            Location::RecycleBin => L::Trash,
            Location::Search { .. } => L::Search,
            Location::Tag(_) => L::Tag,
            Location::Library { .. } => L::Library,
            Location::Shell { .. } => L::Usb,
            Location::DiskUsage(_) => L::ChartPie,
            Location::Downloads => L::ArrowDownToLine,
            Location::Path(p) if rf_win::network::server_root(p).is_some() => L::Server,
            // the archive itself or a folder inside it (name check only: no disk access here, not even for a `.001` name)
            Location::Path(p) if crate::archive_view::is_browsable_path_by_name(p) => L::FileArchive,
            Location::Path(p) => {
                if p.parent().is_none() {
                    L::HardDrive
                } else {
                    L::Folder
                }
            }
        }
    }

    // ---------- navigation ----------

    pub fn navigate(&mut self, to: Location, cx: &mut Context<Self>) {
        if to == Location::RecycleBin {
            self.recycle = None; // fresh listing each visit
        }
        self.history.navigate(to);
        self.filter.query.clear();
        self.load(cx);
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        let from = self.location().clone();
        if self.history.back().is_some() {
            self.select_where_i_came_from(&from, cx);
            self.load(cx);
        }
    }

    /// After going back: the folder one came out of is selected in the folder it is in and scrolled into view, as it is after
    /// going up, and under the same setting ("Select the previous folder when going up"). Issue 40: without it one's place in a
    /// long folder was lost at every Back.
    fn select_where_i_came_from(&mut self, from: &Location, cx: &mut Context<Self>) {
        if cx.global::<crate::settings::Settings>().select_previous_on_up
            && let Some(name) = rf_core::history::came_from(from, self.location())
        {
            self.reselect = Some(name);
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if self.history.forward().is_some() {
            self.load(cx);
        }
    }

    /// Jumps `n` steps through history (from the back/forward button menus).
    pub fn jump(&mut self, back: bool, n: usize, cx: &mut Context<Self>) {
        let from = self.location().clone();
        let moved = if back { self.history.back_n(n) } else { self.history.forward_n(n) };
        if moved {
            if back {
                self.select_where_i_came_from(&from, cx);
            }
            self.load(cx);
        }
    }

    pub fn up(&mut self, cx: &mut Context<Self>) {
        let cur = self.location().clone();
        if let Some(parent) = cur.parent() {
            // Settings → "Select the previous folder when going up"; in the columns layout always, as Left there walks back the way
            // one came (issue 73: "Back (left arrow) always takes you to top of parent folder")
            if self.view_mode == ViewMode::Columns || cx.global::<crate::settings::Settings>().select_previous_on_up {
                self.reselect = cur.path().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            }
            self.navigate(parent, cx);
        }
    }

    fn ensure_column(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if self.column_cache.contains_key(&dir) || !self.column_pending.insert(dir.clone()) {
            return;
        }
        if self.column_cache.len() > 48 {
            self.column_cache.clear();
        }
        // the list's own visibility rules: hidden, system and dot files (issue 51: "Hidden files and dotfiles can't be hidden as
        // they can in other layouts"; the side columns only looked at the hidden attribute)
        let shown = Filter {
            show_hidden: self.filter.show_hidden,
            show_system: self.filter.show_system,
            dot_hidden: self.filter.dot_hidden,
            ..Default::default()
        };
        let d2 = dir.clone();
        let work = cx.background_spawn(async move {
            let mut v = Vec::new();
            let _ = rf_win::list_dir(&d2, 4096, &|| false, &mut |b| v.extend(b));
            v.retain(|e| shown.accepts(e));
            let spec = SortSpec::default();
            v.sort_by(|a, b| spec.compare(a, b));
            v
        });
        cx.spawn(async move |this, cx| {
            let v = work.await;
            let _ = this.update(cx, |t, cx| {
                t.column_pending.remove(&dir);
                t.column_cache.insert(dir, Arc::new(v));
                cx.notify();
            });
        })
        .detach();
    }

    fn render_col_rows(&mut self, range: std::ops::Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let hover_select = cx.global::<crate::settings::Settings>().select_on_hover;
        let p = Palette::for_window(window);
        let mut out = Vec::with_capacity(range.len());
        for vi in range {
            let Some(&ei) = self.visible.get(vi) else { break };
            let e = &self.entries[ei as usize];
            let selected = self.selected.contains(&ei);
            let cat = icons::category(e);
            let shell_icon = self.parent_of(ei).map(|d| d.to_path_buf()).and_then(|d| self.images.update(cx, |c, cx| c.icon(&d, e, cx)));
            let overlay = self.overlay_of(ei, px(16.), cx);
            let e = &self.entries[ei as usize];
            let is_dir = e.is_dir();
            out.push(
                h_flex()
                    .id(("crow", vi))
                    .w_full()
                    .h(px(28.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .text_sm()
                    .relative()
                    .when(selected, |d| d.bg(p.selection).child(theme::selection_pill(&p)))
                    .when(!selected, |d| d.hover(|s| s.bg(p.hover)))
                    .child(download_marked(link_marked(
                        match &shell_icon {
                            Some(im) => img(im.clone()).size(px(16.)).flex_shrink_0().into_any_element(),
                            None => Icon::new(icons::icon_for(cat)).size(px(16.)).text_color(theme::kind_color(cat, p.dark)).into_any_element(),
                        },
                        px(16.),
                        e.link,
                        overlay,
                    ), self.part_frac(ei)))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.name_hit = true)).child(crate::rtl::display_text(&e.name)))
                    .when(is_dir, |d| d.child(Icon::new(L::ChevronRight).size(px(12.)).text_color(p.text_tertiary)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.on_row_mouse_down(vi, ev, window, cx)),
                    )
                    .on_mouse_up(MouseButton::Left, cx.listener(move |this, ev: &MouseUpEvent, _, cx| this.on_row_mouse_up(vi, ev, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| this.on_row_right_down(vi, window, cx)),
                    )
                    .when_some(self.path_of(ei), |d, path| file_drop_target(d, path, is_dir, &p))
                    // Settings → "Select files and folders on hover"
                    .when(hover_select, |d| d.on_hover(cx.listener(move |this, on: &bool, _, cx| this.hover_row(vi, *on, cx))))
                    .into_any_element(),
            );
        }
        out
    }

    fn render_columns(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(cur) = self.current_dir() else { return div().into_any_element() };
        // Up to two ancestor columns, each highlighting the child on the current path.
        let mut ancestors: Vec<(PathBuf, String)> = Vec::new();
        let mut child = cur.clone();
        for _ in 0..2 {
            let Some(parent) = child.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf()) else { break };
            ancestors.push((parent.clone(), rf_core::history::display_name(&child)));
            child = parent;
        }
        ancestors.reverse();
        let next_dir = self.cursor_entry().filter(|&ei| self.entries[ei as usize].is_dir()).and_then(|ei| self.path_of(ei));
        for (d, _) in &ancestors {
            self.ensure_column(d.clone(), cx);
        }
        if let Some(n) = &next_dir {
            self.ensure_column(n.clone(), cx);
        }
        let key = (cur.clone(), next_dir.clone());
        if self.col_key.as_ref() != Some(&key) {
            self.col_key = Some(key);
            let n_cols = ancestors.len() + 1 + usize::from(next_dir.is_some());
            self.col_scroll.scroll_to_item(n_cols.saturating_sub(1));
        }

        let entity = cx.entity();
        let mut strip = h_flex().id("columns").size_full().overflow_x_scroll().track_scroll(&self.col_scroll);
        for (k, (dir, on_path)) in ancestors.into_iter().enumerate() {
            let list = self.column_cache.get(&dir).cloned();
            let handle = self.col_scrolls.entry(dir.clone()).or_insert_with(UniformListScrollHandle::new).clone();
            // Bring the highlighted child into view once its listing is loaded.
            if let Some(l) = &list {
                if self.col_scrolled.insert((dir.clone(), on_path.clone())) {
                    if let Some(i) = l.iter().position(|e| *e.name == *on_path) {
                        handle.scroll_to_item(i, ScrollStrategy::Center);
                    }
                }
            }
            strip = strip.child(static_column(("col-anc", k).into(), dir, list, Some(on_path), Some(handle), p, entity.clone()));
        }
        let count = self.visible.len();
        strip = strip.child(
            div()
                .w(COL_W)
                .flex_shrink_0()
                .h_full()
                .border_r_1()
                .border_color(p.divider)
                .child(
                    uniform_list("col-cur", count, cx.processor(|this, range, window, cx| this.render_col_rows(range, window, cx)))
                        .track_scroll(&self.scroll)
                        .size_full()
                        .p_1(),
                ),
        );
        if let Some(nd) = next_dir {
            let list = self.column_cache.get(&nd).cloned();
            strip = strip.child(static_column("col-next".into(), nd, list, None, None, p, entity));
        }
        strip.into_any_element()
    }

    /// Delete / Shift+Delete while the Recycle Bin page is shown (the folder list has the keyboard, e.g. after the address bar):
    /// the bin page deletes its own selection for good, after its own confirmation. `false` anywhere else.
    fn bin_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match (self.location(), self.recycle.clone()) {
            (Location::RecycleBin, Some(rv)) => {
                rv.update(cx, |r, cx| r.delete_selected(window, cx));
                true
            }
            _ => false,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // (a customised folder's icon may have changed without its time or attributes: ask for those again)
        crate::image_cache::refresh_folder_icons();
        self.column_cache.clear();
        // F5 measures folder sizes afresh (deep changes don't move a folder's own date)
        if let Some(p) = self.location().path() {
            rf_win::foldersize::shared_cache().invalidate(p);
        }
        if matches!(self.location(), Location::Home) {
            self.places.update(cx, |p, cx| p.refresh(cx));
        }
        if let (Location::RecycleBin, Some(rv)) = (self.location(), self.recycle.clone()) {
            rv.update(cx, |r, cx| r.reload(cx));
        }
        // folders whose size was asked for are measured again, not forgotten
        let measured: Vec<PathBuf> = self.folder_sizes.keys().cloned().collect();
        self.load(cx);
        for p in measured {
            self.folder_size(p, cx);
        }
    }

    /// Re-reads git status for the current folder in the background (no-op outside a repo).
    /// [`Self::refresh_git`] for changes on the disk: at most every 2 s (each one runs `git
    /// status`, a process; a folder being written to would start one per change), the last
    /// change always followed by one.
    fn refresh_git_soon(&mut self, cx: &mut Context<Self>) {
        const GAP: std::time::Duration = std::time::Duration::from_secs(2);
        if self.git_soon.is_some() {
            return; // one is already waiting
        }
        let wait = self.git_last.map_or(std::time::Duration::ZERO, |t| GAP.saturating_sub(t.elapsed()));
        if wait.is_zero() {
            self.refresh_git(cx);
            return;
        }
        self.git_soon = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                v.git_soon = None;
                if v.git.is_some() {
                    v.refresh_git(cx);
                }
            });
        }));
    }

    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        self.git_last = Some(Instant::now());
        let Some(dir) = self.location().path().map(|p| p.to_path_buf()).filter(|_| !self.is_search()) else {
            self.git = None;
            return;
        };
        let commits = Self::commit_cols_on(cx);
        let d = dir.clone();
        let work = cx.background_spawn(async move {
            let root = rf_win::git::repo_root(&d)?;
            Some((rf_win::git::status(&root).ok().map(Arc::new)?, root))
        });
        self._git_task = Some(cx.spawn(async move |this, cx| {
            let got = work.await;
            let Ok(Some(log)) = this.update(cx, |v, cx| {
                let (st, root) = got.map_or((None, None), |(s, r)| (Some(s), Some(r)));
                v.git = st.clone();
                cx.notify();
                // the status shows first; the commit columns follow from a slower `git log`
                let (st, root) = (st?, root?);
                commits.then(|| {
                    cx.background_spawn(async move {
                        // untracked / ignored children have no commit: don't scan history for them
                        let names: Vec<String> = std::fs::read_dir(&dir)
                            .map(|rd| {
                                rd.flatten()
                                    .map(|e| e.file_name().to_string_lossy().into_owned())
                                    .filter(|n| n != ".git")
                                    .filter(|n| {
                                        !matches!(
                                            rf_win::git::state_for_child(&st, &dir, n),
                                            Some(rf_win::git::FileState::Untracked | rf_win::git::FileState::Ignored)
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        Arc::new(rf_win::git::last_commits_of_children(&root, &dir, &names, 5_000))
                    })
                })
            }) else {
                return;
            };
            let log = log.await;
            let _ = this.update(cx, |v, cx| {
                v.git_commits = Some(log);
                cx.notify();
            });
        }));
    }

    /// Asks for the overlays of the current folder's items in the background (`rf_win::overlays`), after a listing. Only plain folders
    /// on a disk or a share; links are left out (rustyfiles draws their arrow itself), and a huge folder's first items only.
    fn refresh_overlays(&mut self, cx: &mut Context<Self>) {
        const MOST: usize = 5_000;
        self.overlay_cancel.store(true, Ordering::Relaxed);
        self.overlay_cancel = Arc::new(AtomicBool::new(false));
        let dir = self.location().path().map(|p| p.to_path_buf()).filter(|d| {
            let s = d.to_string_lossy();
            let plain = (s.len() >= 2 && s.as_bytes()[1] == b':') || (s.starts_with(r"\\") && !s.starts_with(r"\\?\"));
            plain && !self.is_search() && !crate::nav_touch::may_hold_archive(d)
        });
        let Some(dir) = dir.filter(|_| cx.global::<crate::settings::Settings>().show_overlays) else {
            self.overlays = None;
            return;
        };
        let names: Vec<String> = self.entries.iter().filter(|e| !e.link).take(MOST).map(|e| e.name.to_string()).collect();
        let cancel = self.overlay_cancel.clone();
        let work = cx.background_spawn(async move { rf_win::overlays::indexes(&dir, names, cancel) });
        self._overlay_task = Some(cx.spawn(async move |this, cx| {
            let got = work.await;
            let _ = this.update(cx, |v, cx| {
                let map: std::collections::HashMap<Arc<str>, i32> = got.into_iter().map(|(n, i)| (Arc::from(n), i)).collect();
                if v.overlays.as_ref().is_none_or(|o| **o != map) {
                    v.overlays = Some(Arc::new(map));
                    cx.notify();
                }
            });
        }));
    }

    /// [`Self::refresh_overlays`] for changes on the disk: once things calm down for a moment (a sync program changes a file's mark
    /// after writing it).
    fn refresh_overlays_soon(&mut self, cx: &mut Context<Self>) {
        if self.overlay_soon.is_some() {
            return;
        }
        self.overlay_soon = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update(cx, |v, cx| {
                v.overlay_soon = None;
                v.refresh_overlays(cx);
            });
        }));
    }

    /// The overlay picture over item `ei`'s icon drawn `size` wide, when another program marks it.
    fn overlay_of(&mut self, ei: u32, size: Pixels, cx: &mut Context<Self>) -> Option<Arc<gpui::RenderImage>> {
        if !cx.global::<crate::settings::Settings>().show_overlays {
            return None;
        }
        let index = {
            let ov = self.overlays.as_ref()?;
            // (the columns view also shows other folders' items)
            if self.parent_of(ei)? != self.location().path()? {
                return None;
            }
            *ov.get(&*self.entries.get(ei as usize)?.name)?
        };
        let device = ((f32::from(overlay_box(size)) * 2.0) as u32).max(32);
        self.images.update(cx, |c, cx| c.overlay(index, device, cx))
    }

    fn commit_cols_on(cx: &App) -> bool {
        let c = &cx.global::<crate::settings::Settings>().columns;
        c.git_message || c.git_author || c.git_date || c.git_hash
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        // (another listing: a click made before it is not the first click of a double click in it)
        self.load_serial += 1;
        // every folder landed on (navigating, Back/Forward, a refresh) is remembered for the palette's folder search
        crate::folder_history::record(self.location());
        self.apply_folder_layout(cx);
        self.images.update(cx, |c, _| c.cancel_pending());
        self.git = None;
        self.git_commits = None;
        self.refresh_git(cx);
        self.overlay_cancel.store(true, Ordering::Relaxed);
        self.overlays = None;
        self._overlay_task = None;
        self.overlay_soon = None;
        self.renaming = None;
        self._rename_sub = None;
        self.renamed = None;
        self.load_inner(false, cx);
        self.start_watch(cx);
        self.warm_labels(cx);
    }

    /// The localized names of the folders in the path (the breadcrumb and the tab title show them) are read from the disk on a
    /// worker, and the view is drawn again when one of them is found. The drawing itself only looks in the cache, so a place that is
    /// slow to answer cannot hold the window: until the answer comes the folders show under their real names.
    fn warm_labels(&self, cx: &mut Context<Self>) {
        let (Location::Path(path) | Location::Search { root: path, .. }) = self.location() else { return };
        let paths = crate::nav_touch::label_paths(path);
        if paths.iter().all(|p| rf_win::localized::is_known(p)) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { rf_win::localized::warm(paths) }).await;
            if found > 0 {
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }

    /// Re-lists the current folder in the background and swaps the result in, keeping
    /// selection and scroll position (used by the folder watcher and after file operations).
    pub fn soft_reload(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.location().path().is_none() || self.is_search() {
            return;
        }
        if self.git.is_some() {
            self.refresh_git_soon(cx);
        }
        self.load_inner(true, cx);
    }

    fn start_watch(&mut self, cx: &mut Context<Self>) {
        self._watch = None;
        self._watch_task = None;
        if self.is_search() {
            return;
        }
        let Some(path) = self.location().path().map(|p| p.to_path_buf()) else { return };
        if rf_win::ftp::is_ftp(&path) {
            return; // no change notifications over FTP; F5 refreshes
        }
        let (tx, mut rx) = mpsc::unbounded::<Option<Vec<(String, Option<Entry>)>>>();
        // a plain folder is patched with what changed; inside an archive it is listed again
        // (read from the names alone: asking the disk here, on the UI thread, held the window for the seconds a WSL distribution
        // needs to start; a folder that is merely named like an archive is just listed again on a change)
        let patchable = !crate::nav_touch::may_hold_archive(&path);
        // Opening the directory handle can block (offline network share): do it off the UI thread.
        let setup = cx.background_spawn(async move {
            let dir = path.clone();
            rf_win::watch::watch_dir_changes(&path, std::time::Duration::from_millis(150), move |ch| {
                // the changed names read from the disk here, on the watcher's own thread (in
                // order, off the UI thread); None: list the whole folder again
                let patch = (patchable && !ch.rescan && !ch.names.is_empty())
                    .then(|| ch.names.into_iter().map(|n| rf_win::stat_entry(&dir, &n).map(|e| (n, e))).collect::<Result<Vec<_>, _>>().ok())
                    .flatten();
                let _ = tx.unbounded_send(patch);
            })
            .map_err(|e| watch_failed(&path, &e))
            .ok()
        });
        self.watch_gen += 1;
        let generation = self.watch_gen;
        self._watch_task = Some(cx.spawn(async move |this, cx| {
            let Some(w) = setup.await else { return };
            let keep = this
                .update(cx, |v, _| {
                    if v.watch_gen == generation {
                        v._watch = Some(w);
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !keep {
                return;
            }
            while let Some(patch) = rx.next().await {
                let r = this.update(cx, |v, cx| match patch {
                    Some(p) if !v.loading && v.soft_pending.is_none() && !v.is_search() => v.apply_patch(p, cx),
                    _ => v.soft_reload(cx),
                });
                if r.is_err() {
                    break;
                }
            }
        }));
    }

    pub(crate) fn selected_is_single_dir(&self) -> bool {
        self.selected.len() == 1 && self.selected.iter().all(|&i| self.entries[i as usize].is_dir())
    }

    fn load_inner(&mut self, soft: bool, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self._load_task = None;
        // a soft reload (watcher) keeps sizes and running walks; a real load starts over
        if !soft {
            self.props_reset();
            self.sortx_reset();
            self.folder_sizes.clear();
            self.size_queue.clear();
            for (b, _) in self.size_batches.drain(..) {
                b.cancel();
            }
        }
        if soft {
            self.soft_pending = Some(Vec::new());
            self.soft_started = Instant::now();
        } else {
            self.soft_pending = None;
            self.entries.clear();
            self.entries_gen += 1;
            self.parents.clear();
            self.shell_items.clear();
            self.shell_title = None;
            self.search_stage = None;
            self.visible.clear();
            self.selected.clear();
            self.cursor = None;
            self.anchor = None;
            self.error = None;
            self.load_ms = None;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            // slow sources (offline share, FTP, big network folder): show a placeholder after a
            // moment instead of a blank pane; fast folders never see it
            self._slow_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SLOW_LOAD).await;
                let _ = this.update(cx, |v, cx| {
                    if v.loading {
                        cx.notify();
                    }
                });
            }));
        }

        if let Location::Library { file, .. } = self.location().clone() {
            self.library_save = self
                .places
                .read(cx)
                .libraries
                .iter()
                .find(|l| l.file == file)
                .and_then(|l| l.default_save.clone().or_else(|| l.folders.first().cloned()))
                .map(|d| (file.clone(), d));
            self.loading = true;
            self.load_started = Instant::now();
            let generation = self.generation;
            let cancel = self.cancel.clone();
            let (tx, mut rx) = mpsc::unbounded::<Msg>();
            std::thread::Builder::new()
                .name("rf-library".into())
                .spawn(move || {
                    let Some(lib) = rf_win::libraries::load(&file) else {
                        let _ = tx.unbounded_send(Msg::Done(Err("Kütüphane açılamadı".into())));
                        return;
                    };
                    let mut total = 0usize;
                    let mut last_err = None;
                    for dir in &lib.folders {
                        let parent: Arc<std::path::Path> = dir.as_path().into();
                        let r = rf_win::list_dir(dir, 512, &|| cancel.load(Ordering::Relaxed), &mut |b| {
                            let ps = vec![parent.clone(); b.len()];
                            let _ = tx.unbounded_send(Msg::Hits(b, ps));
                        });
                        match r {
                            Ok(n) => total += n,
                            Err(e) => last_err = Some(e.to_string()),
                        }
                    }
                    let res = if total == 0 { last_err.map_or(Ok(0), Err) } else { Ok(total) };
                    let _ = tx.unbounded_send(Msg::Done(res));
                })
                .ok();
            self._load_task = Some(cx.spawn(async move |this, cx| {
                let mut msgs = Vec::new();
                while let Some(m) = rx.next().await {
                    msgs.push(m);
                }
                let _ = this.update(cx, |v, cx| v.apply(generation, msgs, cx));
            }));
            cx.notify();
            return;
        }
        if let Location::Shell { parsing, .. } = self.location().clone() {
            self.loading = true;
            self.load_started = Instant::now();
            let generation = self.generation;
            let cancel = self.cancel.clone();
            let (tx, mut rx) = mpsc::unbounded::<Msg>();
            std::thread::Builder::new()
                .name("rf-shell-list".into())
                .spawn(move || {
                    use rf_win::shellfolder as sf;
                    if let Some(n) = sf::display_name(&parsing) {
                        let _ = tx.unbounded_send(Msg::ShellName(n));
                    }
                    let r = sf::list(&parsing, &|| cancel.load(Ordering::Relaxed), &mut |batch| {
                        let es = batch
                            .iter()
                            .map(|c| {
                                let kind = if c.is_folder { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File };
                                Entry::new(&c.name, kind, c.size.unwrap_or(0), c.modified_ms.unwrap_or(0), 0, 0)
                            })
                            .collect();
                        let _ = tx.unbounded_send(Msg::Shell(es, batch));
                    });
                    let _ = tx.unbounded_send(Msg::Done(r.map_err(|e| format!("{e:#}"))));
                })
                .ok();
            self._load_task = Some(cx.spawn(async move |this, cx| {
                while let Some(first) = rx.next().await {
                    let mut msgs = vec![first];
                    while let Ok(m) = rx.try_recv() {
                        msgs.push(m);
                    }
                    if this.update(cx, |v, cx| v.apply(generation, msgs, cx)).is_err() {
                        break;
                    }
                }
            }));
            cx.notify();
            return;
        }
        if let Location::Tag(id) = self.location().clone() {
            let paths = cx.global::<crate::tags::Tags>().0.paths_with(id);
            self.loading = true;
            self.load_started = Instant::now();
            let generation = self.generation;
            let (tx, mut rx) = mpsc::unbounded::<Msg>();
            std::thread::spawn(move || {
                use std::os::windows::fs::MetadataExt;
                let mut es = Vec::new();
                let mut ps: Vec<Arc<std::path::Path>> = Vec::new();
                for p in paths {
                    let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let kind = if m.is_dir() { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File };
                    es.push(Entry::new(
                        &name,
                        kind,
                        m.file_size(),
                        format::filetime_to_unix_ms(m.last_write_time()),
                        format::filetime_to_unix_ms(m.creation_time()),
                        m.file_attributes(),
                    ));
                    ps.push(p.parent().unwrap_or(&p).into());
                }
                let n = es.len();
                let _ = tx.unbounded_send(Msg::Hits(es, ps));
                let _ = tx.unbounded_send(Msg::Done(Ok(n)));
            });
            self._load_task = Some(cx.spawn(async move |this, cx| {
                let mut msgs = Vec::new();
                while let Some(m) = rx.next().await {
                    msgs.push(m);
                }
                let _ = this.update(cx, |v, cx| v.apply(generation, msgs, cx));
            }));
            cx.notify();
            return;
        }
        let Some(path) = self.location().path().map(|p| p.to_path_buf()) else {
            self.loading = false;
            cx.notify();
            return;
        };
        if !soft {
            self.loading = true;
            self.load_started = Instant::now();
        }
        let generation = self.generation;
        let cancel = self.cancel.clone();
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        if rf_win::ftp::is_ftp(&path) {
            let url = path.to_string_lossy().into_owned();
            std::thread::Builder::new()
                .name("rf-ftp".into())
                .spawn(move || {
                    let r = rf_win::ftp::list(&url).map(|es| {
                        let n = es.len();
                        let _ = tx.unbounded_send(Msg::Batch(es));
                        n
                    });
                    let _ = tx.unbounded_send(Msg::Done(r.map_err(|e| e.to_string())));
                })
                .ok();
        } else if let Some(server) = rf_win::network::server_root(&path) {
            std::thread::Builder::new()
                .name("rf-shares".into())
                .spawn(move || {
                    let r = rf_win::network::shares(&server).map(|list| {
                        let es: Vec<Entry> = list
                            .iter()
                            .map(|s| Entry::new(&s.name, rf_core::EntryKind::Dir, 0, 0, 0, 0x10))
                            .collect();
                        let n = es.len();
                        let _ = tx.unbounded_send(Msg::Batch(es));
                        n
                    });
                    let _ = tx.unbounded_send(Msg::Done(r.map_err(|e| e.to_string())));
                })
                .ok();
        } else if let Location::Search { root, query } = self.location().clone() {
            let include_hidden = self.filter.show_hidden;
            let use_index = cx.global::<crate::settings::Settings>().search_index;
            // "tag:Kırmızı" / "etiket:kırmızı": tagged items under this folder (Files' tag: query)
            let tag_paths: Option<Vec<PathBuf>> = {
                let lower = rf_core::entry::fold_case(&query);
                lower.strip_prefix("tag:").or_else(|| lower.strip_prefix("etiket:")).map(|expr| {
                    // "tag:kırmızı AND iş", "tag:kırmızı OR mavi NOT bitti" (left to right)
                    let store = &cx.global::<crate::tags::Tags>().0;
                    let with = |name: &str| -> Vec<PathBuf> {
                        store
                            .defs
                            .iter()
                            .filter(|d| rf_core::entry::fold_case(&d.name) == name || rf_core::entry::fold_case(&crate::i18n::td(&d.name)) == name)
                            .flat_map(|d| store.paths_with(d.id))
                            .filter(|p| p.starts_with(&root))
                            .collect()
                    };
                    let all = || -> Vec<PathBuf> { store.defs.iter().flat_map(|d| store.paths_with(d.id)).filter(|p| p.starts_with(&root)).collect() };
                    crate::tags::eval_query(&crate::tags::parse_query(expr), &with, &all)
                })
            };
            std::thread::Builder::new()
                .name("rf-search".into())
                .spawn(move || {
                    if let Some(paths) = tag_paths {
                        let mut es = Vec::new();
                        let mut ps: Vec<Arc<std::path::Path>> = Vec::new();
                        for p in paths {
                            use std::os::windows::fs::MetadataExt;
                            let Ok(m) = std::fs::metadata(&p) else { continue };
                            let ms = |t: std::io::Result<std::time::SystemTime>| {
                                t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
                            };
                            let kind = if m.is_dir() { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File };
                            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            es.push(Entry::new(&name, kind, if m.is_dir() { 0 } else { m.len() }, ms(m.modified()), ms(m.created()), m.file_attributes()));
                            ps.push(p.parent().unwrap_or(&p).into());
                        }
                        let n = es.len();
                        let _ = tx.unbounded_send(Msg::Hits(es, ps));
                        let _ = tx.unbounded_send(Msg::Done(Ok(n)));
                        return;
                    }
                    // 1. our name index (milliseconds) when it covers this folder
                    let glob = query.contains(['*', '?']);
                    let mut seen: Option<std::collections::HashSet<String>> = None;
                    let mut found = 0usize;
                    let t_index = std::time::Instant::now();
                    let _ = tx.unbounded_send(Msg::Stage(SearchStage::Index));
                    // (Windows Search gave up on: the walk keeps saying so)
                    let mut windows_silent = false;
                    if let Some(paths) = rf_win::indexer::search(&query, &root, include_hidden, 50_000) {
                        let looked_up = t_index.elapsed();
                        found = send_index_hits(&paths, &tx, &cancel);
                        log::info!("search: the index answered \"{query}\" in {} with {} hits ({looked_up:?} to find, {:?} with details)", root.display(), found, t_index.elapsed());
                        seen = Some(paths.iter().map(|p| p.to_string_lossy().to_lowercase()).collect());
                    }
                    // 2. Windows Search (names + contents + AQS) where the folder is indexed: with
                    //    our index, only what it adds (matches inside files)
                    // (both asked of Windows' search service, which can hang where it is broken or switched off: issue 67, "it just
                    // says loading": each answer is waited for only so long, then the disk is walked as where nothing is indexed)
                    let indexed = use_index && !glob && {
                        let r = root.clone();
                        within(Duration::from_secs(3), move || rf_win::wsearch::is_indexed(&r)).unwrap_or_else(|| {
                            log::warn!("search: Windows Search did not say in 3 s whether {} is indexed; walking instead", root.display());
                            false
                        })
                    };
                    if indexed {
                        let _ = tx.unbounded_send(Msg::Stage(SearchStage::Windows));
                        let tx2 = tx.clone();
                        let seen2 = seen.clone();
                        let (root2, query2, cancel2) = (root.clone(), query.clone(), cancel.clone());
                        // its own stop, so that a search that gives nothing for long can be left
                        let given_up = Arc::new(AtomicBool::new(false));
                        let given_up2 = given_up.clone();
                        let delivered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                        let delivered2 = delivered.clone();
                        let (done_tx, done_rx) = std::sync::mpsc::channel();
                        let _ = std::thread::Builder::new().name("rf-wsearch".into()).spawn(move || {
                            let stop = || cancel2.load(Ordering::Relaxed) || given_up2.load(Ordering::Relaxed);
                            let r = rf_win::wsearch::search(&root2, &query2, 50_000, &stop, &mut |hits| {
                                delivered2.fetch_add(hits.len().max(1), Ordering::Relaxed);
                                let mut es = Vec::with_capacity(hits.len());
                                let mut ps = Vec::with_capacity(hits.len());
                                for h in hits {
                                    if !include_hidden && h.entry.is_hidden() {
                                        continue;
                                    }
                                    if seen2.as_ref().is_some_and(|s| s.contains(&h.path.to_string_lossy().to_lowercase())) {
                                        continue;
                                    }
                                    let parent: Arc<std::path::Path> = h.path.parent().unwrap_or(&h.path).into();
                                    ps.push(parent);
                                    es.push(h.entry);
                                }
                                let _ = tx2.unbounded_send(Msg::Hits(es, ps));
                            });
                            let _ = done_tx.send(r.map(|n| n as usize).map_err(|e| format!("{e:#}")));
                        });
                        let started = std::time::Instant::now();
                        let r = loop {
                            match done_rx.recv_timeout(Duration::from_millis(250)) {
                                Ok(r) => break Some(r),
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Some(Err("the Windows Search thread ended".into())),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            }
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            if delivered.load(Ordering::Relaxed) == 0 && started.elapsed() >= WSEARCH_SILENCE {
                                given_up.store(true, Ordering::Relaxed);
                                let _ = tx.unbounded_send(Msg::Stage(SearchStage::WindowsSilent));
                                windows_silent = true;
                                log::warn!("search: Windows Search gave nothing for \"{query}\" in {} s; walking instead", WSEARCH_SILENCE.as_secs());
                                break None;
                            }
                        };
                        match r {
                            Some(Ok(n)) => {
                                let _ = tx.unbounded_send(Msg::Done(Ok(n + found)));
                                return;
                            }
                            Some(Err(e)) if seen.is_none() => log::warn!("index search failed, walking instead: {e}"),
                            Some(Err(e)) => log::warn!("Windows Search failed: {e}"),
                            None => {}
                        }
                    }
                    if seen.is_some() {
                        let _ = tx.unbounded_send(Msg::Done(Ok(found)));
                        return;
                    }
                    // 3. walking the disk
                    if !windows_silent {
                        let _ = tx.unbounded_send(Msg::Stage(SearchStage::Walk));
                    }
                    let q = rf_win::search::SearchQuery { root, pattern: query, include_hidden, max_results: 50_000 };
                    let tx2 = tx.clone();
                    let r = rf_win::search::search(&q, &|| cancel.load(Ordering::Relaxed), &mut |hits| {
                        let mut es = Vec::with_capacity(hits.len());
                        let mut ps = Vec::with_capacity(hits.len());
                        for h in hits {
                            let parent: Arc<std::path::Path> = h.path.parent().unwrap_or(&h.path).into();
                            ps.push(parent);
                            es.push(h.entry);
                        }
                        let _ = tx2.unbounded_send(Msg::Hits(es, ps));
                    });
                    let _ = tx.unbounded_send(Msg::Done(r.map(|s| s.hits as usize).map_err(|e| e.to_string())));
                })
                .ok();
        } else {
        let spec = (self.filter.clone(), self.sort, self.group);
        std::thread::Builder::new()
            .name("rf-list".into())
            .spawn(move || {
                let tx2 = tx.clone();
                let mut all: Vec<Entry> = Vec::new();
                let r = match crate::archive_view::split(&path) {
                    // inside an archive: list its entries like a folder
                    // a protected archive asks for its password first (zip lists without one,
                    // but every open/copy would fail later)
                    Some((archive, inner)) => match crate::archive_view::password_gate(&archive) {
                        Some(wrong) => Err(format!("{}{}{}", crate::archive_view::PASSWORD_MARK, wrong as u8, archive.display())),
                        None => crate::archive_view::list_inside(&archive, &inner)
                            .map(|es| {
                                let n = es.len();
                                all.extend(es.iter().cloned());
                                let _ = tx2.unbounded_send(Msg::Batch(es));
                                n
                            })
                            .map_err(|e| match crate::archive_view::password_problem(&e) {
                                Some(wrong) => format!("{}{}{}", crate::archive_view::PASSWORD_MARK, wrong as u8, archive.display()),
                                None => format!("{e:#}"),
                            }),
                    },
                    None => rf_win::list_dir(&path, 512, &|| cancel.load(Ordering::Relaxed), &mut |b| {
                        all.extend(b.iter().cloned());
                        let _ = tx2.unbounded_send(Msg::Batch(b));
                    })
                    .map_err(|e| e.to_string()),
                };
                // (a sort by tag or path is made from ranks the view makes after the listing: a by-name view
                // made here would be thrown away)
                if r.is_ok() && !cancel.load(Ordering::Relaxed) && !spec.1.key.needs_ranks() {
                    let _ = tx.unbounded_send(Msg::View(compute_view(&all, &spec.0, spec.1, spec.2, None)));
                }
                let _ = tx.unbounded_send(Msg::Done(r.map_err(|e| e.to_string())));
            })
            .ok();
        }

        self._load_task = Some(cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut msgs = vec![first];
                while let Ok(m) = rx.try_recv() {
                    msgs.push(m);
                }
                let alive = this.update(cx, |v, cx| v.apply(generation, msgs, cx)).is_ok();
                if !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn apply(&mut self, generation: u64, msgs: Vec<Msg>, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        if self.soft_pending.is_some() {
            self.apply_soft(msgs, cx);
            return;
        }
        let mut done = false;
        let mut view = None;
        for m in msgs {
            match m {
                Msg::View(v) => view = Some(v),
                Msg::Batch(b) => {
                    self.entries.extend(b);
                    self.entries_gen += 1;
                }
                Msg::Hits(es, ps) => {
                    self.entries.extend(es);
                    self.entries_gen += 1;
                    self.parents.extend(ps);
                }
                Msg::Shell(es, items) => {
                    self.entries.extend(es);
                    self.entries_gen += 1;
                    self.shell_items.extend(items);
                }
                Msg::ShellName(n) => self.shell_title = Some(n),
                Msg::Stage(s) => self.search_stage = Some(s),
                Msg::Done(r) => {
                    done = true;
                    self.loading = false;
                    self.search_stage = None;
                    self.apply_adaptive();
                    self.load_ms = Some(self.load_started.elapsed().as_millis());
                    if trace_live() {
                        log::info!("live: listed {} entries in {} ms", self.entries.len(), self.load_started.elapsed().as_millis());
                    }
                    if let Err(e) = r {
                        if let Some(rest) = e.strip_prefix(crate::archive_view::PASSWORD_MARK) {
                            // "<0|1><archive path>": ask the workspace to prompt for it
                            let wrong = rest.starts_with('1');
                            let archive = PathBuf::from(&rest[1..]);
                            self.error = Some(t!("Bu arşiv parola ile korunuyor").to_string());
                            cx.emit(FolderEvent::NeedPassword(archive, wrong));
                        } else if e != "İptal edildi" {
                            self.error = Some(e);
                        }
                    }
                }
            }
        }
        // Small folders: keep fully sorted while streaming. Huge folders: append now, sort once at the end.
        if done && self.try_install_view(view) {
            // sorted on the listing thread
        } else if done || self.entries.len() <= 20_000 || self.group != GroupBy::None {
            self.rebuild_visible();
        } else {
            let start = self.visible.len() as u32;
            self.visible.extend(start..self.entries.len() as u32);
        }
        if done {
            self.resort_for_sizes(cx);
            self.refresh_overlays(cx);
            self.apply_reselect(cx);
            // a sort by tag or path makes its ranks now that the listing is complete
            self.sortx_ensure(cx);
        }
        cx.notify();
    }

    fn apply_reselect(&mut self, cx: &mut Context<Self>) {
        if let Some((names, until)) = self.reselect_many.take() {
            let want: HashSet<&str> = names.iter().map(String::as_str).collect();
            let found: Vec<usize> = self.visible.iter().enumerate().filter(|&(_, &i)| want.contains(&*self.entries[i as usize].name)).map(|(vi, _)| vi).collect();
            if !found.is_empty() {
                self.selected.clear();
                for &vi in &found {
                    self.selected.insert(self.visible[vi]);
                }
                self.cursor = found.first().copied();
                self.anchor = found.first().copied();
                self.scroll.scroll_to_item(self.list_row(found[0]), ScrollStrategy::Center);
                cx.notify();
            }
            // keep waiting for the rest while the watcher catches up
            if found.len() < want.len() && Instant::now() < until {
                self.reselect_many = Some((names, until));
            }
        }
        // (the listing no longer holds the old name of a rename that was under way: it landed, and the rows show their own names)
        if let Some((old, _)) = &self.renamed
            && !self.entries.iter().any(|e| *e.name == **old)
        {
            self.renamed = None;
        }
        if let Some(name) = self.reselect.take() {
            if let Some(vi) = self.visible.iter().position(|&i| &*self.entries[i as usize].name == name) {
                self.select_only(vi);
                self.scroll.scroll_to_item(self.list_row(vi), ScrollStrategy::Nearest);
                if std::mem::take(&mut self.rename_after_load) {
                    self.pending_rename = true;
                }
                if let Some(h) = self.menu_after_load.take() {
                    let me = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        let _ = h.update(cx, |_, window, cx| me.update(cx, |v, cx| v.show_context_menu(false, window, cx)));
                    });
                }
            } else if self.rename_after_load {
                // not there yet (watcher lag) — keep waiting for the next refresh
                self.reselect = Some(name);
            }
        }
        // in the columns layout a folder is entered on its first item, as in yazi (issue 51: "When entering a folder, you first have
        // to press down on the arrow keys to focus on the top entry"): Right goes on into it at once
        if self.view_mode == ViewMode::Columns && self.cursor.is_none() && self.reselect.is_none() && !self.loading && !self.visible.is_empty() {
            self.select_only(0);
        }
        let _ = cx;
    }

    fn apply_soft(&mut self, msgs: Vec<Msg>, cx: &mut Context<Self>) {
        let mut done = false;
        let mut view = None;
        for m in msgs {
            match m {
                Msg::View(v) => view = Some(v),
                Msg::Batch(b) => {
                    if let Some(p) = self.soft_pending.as_mut() {
                        p.extend(b)
                    }
                }
                Msg::Hits(..) | Msg::Shell(..) | Msg::ShellName(_) | Msg::Stage(_) => {}
                Msg::Done(Err(e)) if e.contains("bulunamadı") => {
                    // The folder we are showing was deleted/renamed away: step up.
                    self.soft_pending = None;
                    if let Some(parent) = self.location().parent() {
                        self.navigate(parent, cx);
                    }
                    return;
                }
                Msg::Done(_) => done = true,
            }
        }
        if !done {
            return;
        }
        let Some(new_entries) = self.soft_pending.take() else { return };
        self.swap_in(new_entries, view, None, cx);
    }

    /// Changed entries read from the disk (the folder watcher): `found` is each changed name
    /// with what is there now (None: gone). The rest of the listing is kept; the sort order is
    /// patched too (no relisting, no sort: a file added to a 100,000-file folder costs a pass).
    fn apply_patch(&mut self, found: Vec<(String, Option<Entry>)>, cx: &mut Context<Self>) {
        fn fold(s: &str) -> std::borrow::Cow<'_, str> {
            if s.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase()) { s.into() } else { s.to_lowercase().into() }
        }
        if self.git.is_some() {
            self.refresh_git_soon(cx);
        }
        // (Windows names ignore case: a rename that only changes case reports the old spelling)
        let mut gone: HashSet<String> = found.iter().map(|(n, _)| fold(n).into_owned()).collect();
        gone.extend(found.iter().filter_map(|(_, e)| e.as_ref()).map(|e| fold(&e.name).into_owned()));
        // (a sort by tag keeps the ranks of the changed files, so that a modified tagged file stays where it was)
        let old_ranks = self.sortx_fresh_ranks();
        let mut gone_ranks: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let old_n = self.entries.len();
        let mut remap = vec![u32::MAX; old_n];
        let mut fresh = Vec::with_capacity(old_n + found.len());
        for (i, e) in self.entries.iter().enumerate() {
            if gone.contains(fold(&e.name).as_ref()) {
                if let Some(r) = &old_ranks {
                    gone_ranks.insert(fold(&e.name).into_owned(), r[i]);
                }
                continue;
            }
            remap[i] = fresh.len() as u32;
            fresh.push(e.clone());
        }
        let kept = fresh.len();
        let mut added = HashSet::new();
        for e in found.into_iter().filter_map(|(_, e)| e) {
            if added.insert(e.name.clone()) {
                fresh.push(e);
            }
        }
        let sorted = match &self.sorted_cache {
            // (a sort by tag or path has its ranks by entry index: the order is made again, not patched)
            Some((sort, generation, s)) if *sort == self.sort && *generation == self.entries_gen && s.len() == old_n && !sort.key.needs_ranks() => {
                let mut v: Vec<u32> = s.iter().filter_map(|&i| Some(remap[i as usize]).filter(|&j| j != u32::MAX)).collect();
                for j in kept..fresh.len() {
                    let pos = v.partition_point(|&x| self.sort.compare(&fresh[x as usize], &fresh[j]) == std::cmp::Ordering::Less);
                    v.insert(pos, j as u32);
                }
                Some(v)
            }
            // a sort by tag or path: the order is patched from the ranks of the old entries (the new ones
            // come in by their rank) instead of sorted again on this thread; new ranks are made right after
            Some((sort, generation, s)) if *sort == self.sort && *generation == self.entries_gen && s.len() == old_n && sort.key.needs_ranks() => {
                old_ranks.as_ref().map(|r| {
                    rf_core::sortkeys::patch_ranked(&self.sort, &fresh, kept, s, &remap, r, |e| {
                        gone_ranks.get(fold(&e.name).as_ref()).copied().unwrap_or(rf_core::sortkeys::NO_RANK)
                    })
                })
            }
            _ => None,
        };
        self.soft_started = Instant::now();
        self.swap_in(fresh, None, sorted, cx);
    }

    /// Puts a new listing of the same folder in place, keeping the selection, the cursor and a
    /// rename by name. `sorted`: the new entries' sort order when already known.
    fn swap_in(&mut self, new_entries: Vec<Entry>, view: Option<View>, sorted: Option<Vec<u32>>, cx: &mut Context<Self>) {
        let (was_selected, applying) = (self.selected.len(), Instant::now());
        let selected_names: HashSet<Arc<str>> =
            self.selected.iter().map(|&i| self.entries[i as usize].name.clone()).collect();
        let cursor_name = self.cursor.and_then(|c| self.visible.get(c)).map(|&i| self.entries[i as usize].name.clone());
        let renaming_name = self.renaming.as_ref().map(|(i, _)| self.entries[*i as usize].name.clone());
        // (a grouping by a property keeps each file's group by name until the groups are made again)
        self.props_before_swap();
        // (a patched order needs no carried ranks: it is installed as it is)
        if sorted.is_none() {
            self.sortx_before_swap();
        }
        self.entries = new_entries;
        self.entries_gen += 1;
        self.refresh_overlays_soon(cx);
        if let Some(s) = sorted {
            self.sorted_cache = Some((self.sort, self.entries_gen, Arc::new(s)));
        }
        if !self.try_install_view(view) {
            self.rebuild_visible();
        }
        // measured folder sizes go back into the fresh entries (before the cursor is placed)
        self.resort_for_sizes(cx);
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| selected_names.contains(&e.name))
            .map(|(i, _)| i as u32)
            .collect();
        if trace_live() {
            log::info!(
                "live: update of {} entries, relisted in {} ms, applied in {} ms; selected {} of {}, cursor {:?}",
                self.entries.len(),
                self.soft_started.elapsed().as_millis(),
                applying.elapsed().as_millis(),
                self.selected.len(),
                was_selected,
                self.cursor
            );
        }
        // the cursor's file renamed or deleted in the background: keep the place, not lose it
        let old_cursor = self.cursor;
        self.cursor = cursor_name
            .and_then(|n| self.visible.iter().position(|&i| self.entries[i as usize].name == n))
            .or_else(|| old_cursor.filter(|_| !self.visible.is_empty()).map(|c| c.min(self.visible.len() - 1)));
        if let Some(n) = renaming_name {
            match self.entries.iter().position(|e| e.name == n) {
                Some(i) => {
                    if let Some(r) = self.renaming.as_mut() {
                        r.0 = i as u32
                    }
                }
                None => {
                    self.renaming = None;
                    self._rename_sub = None;
                }
            }
        }
        self.apply_reselect(cx);
        // a sort or grouping by a property column is made again for the new entries
        self.props_entries_changed(cx);
        self.sortx_ensure(cx);
        cx.notify();
    }

    fn rebuild_visible(&mut self) {
        let view = {
            let tags = self.tag_snap.clone();
            let group = self.group;
            let key = |i: u32| -> (u32, String) {
                match group {
                    GroupBy::Tag => self.path_of(i).and_then(|p| tags.get(&p).cloned()).unwrap_or((u32::MAX, "Etiketsiz".into())),
                    GroupBy::Property => self.props_group_key(i),
                    _ => (0, self.parent_of(i).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
                }
            };
            let extra: Extra = if group.needs_extra() { Some(&key) } else { None };
            match &self.sorted_cache {
                // the same entries in the same order: only the filter or grouping changed
                Some((sort, generation, sorted)) if *sort == self.sort && *generation == self.entries_gen && sorted.len() == self.entries.len() => {
                    view_from_sorted(&self.entries, sorted.clone(), &self.filter, self.sort, self.group, extra)
                }
                _ => {
                    // (ranks for a sort by tag or path; None for every other key)
                    let ranks = self.sortx_ranks();
                    compute_view_ranked(&self.entries, &self.filter, self.sort, self.group, extra, ranks.as_deref().map(|r| r.as_slice()))
                }
            }
        };
        self.install_view(view);
    }

    /// Path → (order, name) of the first tag of every tagged item (for grouping by tag).
    fn refresh_tag_snap(&mut self, cx: &App) {
        let store = &cx.global::<crate::tags::Tags>().0;
        let mut map = std::collections::HashMap::new();
        for (order, d) in store.defs.iter().enumerate() {
            for p in store.paths_with(d.id) {
                map.entry(p).or_insert((order as u32, d.name.clone()));
            }
        }
        self.tag_snap = Arc::new(map);
    }

    /// How far the download behind an unfinished file (`.hdpart`) is, while HyperDownload's card follows it.
    fn part_frac(&self, ei: u32) -> Option<f32> {
        let name = &self.entries.get(ei as usize)?.name;
        if name.len() < 7 || !name[name.len() - 7..].eq_ignore_ascii_case(".hdpart") {
            return None;
        }
        let f = crate::hyperdownload::part_progress(&self.path_of(ei)?)?;
        self.parts_shown.set(true);
        Some(f)
    }

    /// Whether the folder is shown in groups (Details or Grid with a grouping): the "all groups" button and key apply.
    pub fn has_groups(&self) -> bool {
        matches!(self.view_mode, ViewMode::Details | ViewMode::Grid) && self.rows.iter().any(|r| matches!(r, Row::Header(..)))
    }

    /// Whether every group is folded.
    pub fn all_groups_folded(&self) -> bool {
        let mut headers = self.rows.iter().filter_map(|r| if let Row::Header(label, _) = r { Some(label) } else { None }).peekable();
        headers.peek().is_some() && headers.all(|l| self.collapsed.contains(l.as_ref()))
    }

    /// Folds every group, or opens them all again when all are folded (issue 181: "collapse them all at once and then expand the
    /// ones you are interested in").
    pub fn toggle_all_groups(&mut self, cx: &mut Context<Self>) {
        if !self.has_groups() {
            return;
        }
        if self.all_groups_folded() {
            self.collapsed.clear();
        } else {
            let labels: Vec<String> = self.rows.iter().filter_map(|r| if let Row::Header(label, _) = r { Some(label.to_string()) } else { None }).collect();
            self.collapsed.extend(labels);
        }
        self.rebuild_visible();
        cx.notify();
    }

    /// A click on a group's header: the group folds away, or comes back.
    fn toggle_group(&mut self, label: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(label) {
            self.collapsed.insert(label.to_string());
        }
        self.rebuild_visible();
        cx.notify();
    }

    /// Uses a view computed elsewhere if it still matches the current entries and settings.
    fn try_install_view(&mut self, view: Option<View>) -> bool {
        match view {
            // (a grouping that needs the tag store or the parents was not computed off this thread)
            Some(v) if v.len == self.entries.len() && v.spec == (self.filter.clone(), self.sort, self.group) && !self.group.needs_extra() => {
                self.install_view(v);
                true
            }
            _ => false,
        }
    }

    fn install_view(&mut self, view: View) {
        // (installed only when it matches the current entries: see try_install_view)
        self.sorted_cache = Some((view.spec.1, self.entries_gen, view.sorted));
        self.rows = view.rows;
        self.row_of_vi = view.row_of_vi;
        self.visible = view.visible;
        // folded groups: their header stays, their items are not in the list
        if !self.collapsed.is_empty() && !self.rows.is_empty() && matches!(self.view_mode, ViewMode::Details | ViewMode::Grid) {
            let (mut rows, mut row_of_vi, mut visible) = (Vec::with_capacity(self.rows.len()), Vec::new(), Vec::new());
            let mut hide = false;
            for r in std::mem::take(&mut self.rows) {
                match r {
                    Row::Header(label, n) => {
                        hide = self.collapsed.contains(label.as_ref());
                        rows.push(Row::Header(label, n));
                    }
                    Row::Item(vi) if !hide => {
                        row_of_vi.push(rows.len());
                        rows.push(Row::Item(visible.len()));
                        visible.push(self.visible[vi]);
                    }
                    Row::Item(_) => {}
                }
            }
            (self.rows, self.row_of_vi, self.visible) = (rows, row_of_vi, visible);
        }
        // Never act on items the user can't see (filtered out / hidden).
        if !self.selected.is_empty() {
            let vis: HashSet<u32> = self.visible.iter().copied().collect();
            self.selected.retain(|e| vis.contains(e));
        }
        if let Some(c) = self.cursor {
            if c >= self.visible.len() {
                self.cursor = None;
            }
        }
    }

    pub fn set_group(&mut self, g: GroupBy, cx: &mut Context<Self>) {
        self.props_drop_group();
        self.group = g;
        self.collapsed.clear();
        if g == GroupBy::Tag {
            self.refresh_tag_snap(cx);
        }
        self.rebuild_visible();
        self.remember_layout(cx);
        cx.notify();
    }

    fn layout_now(&self) -> crate::layouts::FolderLayout {
        crate::layouts::FolderLayout {
            view: match self.view_mode {
                ViewMode::Details => 0,
                ViewMode::Grid => match self.grid_style {
                    GridStyle::Tiles => 1,
                    GridStyle::Cards => 3,
                    GridStyle::List => 4,
                },
                ViewMode::Columns => 2,
            },
            sort: self.sort.key,
            desc: self.sort.dir == SortDir::Desc,
            dirs_first: self.sort.dirs_first,
            files_first: self.sort.files_first,
            row_size: self.row_size,
            grid_size: self.grid_size,
            group: self.group.index(),
        }
    }

    /// "Make this the default for all folders": the folder's view, sort and grouping become the
    /// Settings defaults (folders with their own remembered layout keep it).
    pub fn make_layout_default(&mut self, cx: &mut Context<Self>) {
        use crate::settings::DefaultView;
        let l = self.layout_now();
        let group = GroupBy::ALL[GroupBy::from_index(l.group).index() as usize].1.to_string();
        crate::settings::update(cx, move |s| {
            s.default_view = match l.view {
                1 | 3 | 4 => DefaultView::Grid,
                2 => DefaultView::Columns,
                _ => DefaultView::Details,
            };
            // (the keys added later - tag, path, sync - belong to one folder's layout; the settings file
            // keeps only the keys every build reads, so the default stays as it was)
            if !l.sort.is_extra() {
                s.default_sort = l.sort;
                s.default_sort_desc = l.desc;
            }
            s.folders_first = l.dirs_first;
            s.files_first = l.files_first;
            s.default_group = group;
        });
    }

    /// Per-folder memory: the user changed the view, sort or grouping of this folder.
    fn remember_layout(&mut self, cx: &mut Context<Self>) {
        self.adaptive_pending = false;
        // "the same layout in all folders": one layout, kept for every new tab and the next start (Threads report: the size went back
        // to the second step after a restart; nothing was saved at all)
        if cx.global::<crate::settings::Settings>().sync_layout {
            let l = self.layout_now();
            crate::layouts::set(std::path::Path::new(SHARED_LAYOUT), l, cx);
            return;
        }
        match self.location().clone() {
            Location::Path(p) => {
                let l = self.layout_now();
                crate::layouts::set(&p, l, cx);
            }
            // every search shares one (issue 165: "Would be nice if it remembered the last time I searched I asked for the results to
            // be sorted by date")
            Location::Search { .. } => {
                let l = self.layout_now();
                crate::layouts::set(std::path::Path::new(SEARCH_LAYOUT), l, cx);
            }
            _ => {}
        }
    }

    /// On entering a folder: its remembered layout, else the defaults from Settings (and the
    /// adaptive choice once the listing is in). With "sync layout" on the tab keeps its own.
    fn apply_folder_layout(&mut self, cx: &mut Context<Self>) {
        let was_columns = self.view_mode == ViewMode::Columns;
        self.apply_folder_layout_inner(cx);
        // Columns is a way of walking the tree, not a look of one folder: it stays on from folder to folder (issue 51: going up to
        // a shallow folder switched back to the details layout, the folder's own remembered one, and the arrow keys stopped walking).
        // Another layout chosen from the menu leaves it, and the remembered layouts are used again from there.
        if was_columns {
            self.view_mode = ViewMode::Columns;
        }
    }

    fn apply_folder_layout_inner(&mut self, cx: &mut Context<Self>) {
        let s = cx.global::<crate::settings::Settings>().clone();
        let first = !std::mem::replace(&mut self.layout_seeded, true);
        if s.sync_layout {
            // the tab keeps its own from folder to folder; a new tab (and every tab at a start) begins with the shared one
            if first && let Some(l) = crate::layouts::get(std::path::Path::new(SHARED_LAYOUT), cx) {
                self.apply_layout_values(l, false, cx);
            }
            return;
        }
        // search results: as the last search was sorted and laid out (else as the folder searched in, the tab's)
        if matches!(self.location(), Location::Search { .. }) {
            if let Some(l) = crate::layouts::get(std::path::Path::new(SEARCH_LAYOUT), cx) {
                self.apply_layout_values(l, false, cx);
            }
            return;
        }
        let Location::Path(p) = self.location().clone() else { return };
        let (l, adaptive) = match crate::layouts::get(&p, cx) {
            Some(l) => (l, false),
            // Adaptive, like Explorer's folder types: Pictures and Videos as a grid, Downloads
            // newest first in date groups (the rest is decided once the folder is listed)
            None if s.default_view == crate::settings::DefaultView::Adaptive && known_kind(&p).is_some() => {
                let grid = known_kind(&p) != Some(KnownKind::Downloads);
                (
                    crate::layouts::FolderLayout {
                        view: if grid { 1 } else { 0 },
                        sort: if grid { s.default_sort } else { rf_core::SortKey::Modified },
                        desc: if grid { s.default_sort_desc } else { true },
                        dirs_first: s.folders_first,
                        files_first: !s.folders_first && s.files_first,
                        row_size: DEFAULT_ROW_SIZE,
                        grid_size: DEFAULT_GRID_SIZE,
                        group: if grid { crate::settings_view::group_ix(&s.default_group) as u8 } else { 3 },
                    },
                    false,
                )
            }
            None => (
                crate::layouts::FolderLayout {
                    view: match s.default_view {
                        crate::settings::DefaultView::Grid => 1,
                        crate::settings::DefaultView::Columns => 2,
                        _ => 0,
                    },
                    sort: s.default_sort,
                    desc: s.default_sort_desc,
                    dirs_first: s.folders_first,
                    files_first: !s.folders_first && s.files_first,
                    row_size: DEFAULT_ROW_SIZE,
                    grid_size: DEFAULT_GRID_SIZE,
                    group: crate::settings_view::group_ix(&s.default_group) as u8,
                },
                s.default_view == crate::settings::DefaultView::Adaptive,
            ),
        };
        self.apply_layout_values(l, adaptive, cx);
    }

    fn apply_layout_values(&mut self, l: crate::layouts::FolderLayout, adaptive: bool, cx: &mut Context<Self>) {
        self.view_mode = match l.view {
            1 | 3 | 4 => ViewMode::Grid,
            2 => ViewMode::Columns,
            _ => ViewMode::Details,
        };
        self.grid_style = match l.view {
            3 => GridStyle::Cards,
            4 => GridStyle::List,
            _ => GridStyle::Tiles,
        };
        self.sort = SortSpec { key: l.sort, dir: if l.desc { SortDir::Desc } else { SortDir::Asc }, dirs_first: l.dirs_first, files_first: l.files_first };
        self.row_size = l.row_size.min(ROW_SIZES.len() as u8 - 1);
        self.grid_size = l.grid_size.min(GRID_SIZES.len() as u8 - 1);
        self.group = GroupBy::from_index(l.group);
        self.collapsed.clear();
        if self.group == GroupBy::Tag {
            self.refresh_tag_snap(cx);
        }
        self.adaptive_pending = adaptive;
    }

}

/// Known folders with a layout of their own in adaptive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownKind {
    Pictures,
    Downloads,
}

/// Pictures and Videos (and folders inside them), or Downloads itself.
fn known_kind(p: &std::path::Path) -> Option<KnownKind> {
    use rf_win::known::KnownFolder;
    static PATHS: std::sync::OnceLock<Vec<(std::path::PathBuf, KnownKind)>> = std::sync::OnceLock::new();
    let paths = PATHS.get_or_init(|| {
        [(KnownFolder::Pictures, KnownKind::Pictures), (KnownFolder::Videos, KnownKind::Pictures), (KnownFolder::Downloads, KnownKind::Downloads)]
            .into_iter()
            .filter_map(|(k, kind)| k.path().map(|p| (p, kind)))
            .collect()
    });
    let lower = |q: &std::path::Path| q.to_string_lossy().to_lowercase();
    let here = lower(p);
    paths.iter().find_map(|(root, kind)| {
        let r = lower(root);
        match kind {
            KnownKind::Downloads => (here == r).then_some(*kind),
            KnownKind::Pictures => (here == r || here.starts_with(&format!("{r}\\"))).then_some(*kind),
        }
    })
}

impl FolderView {
    /// Adaptive layout: a folder of mostly pictures and videos opens as a grid.
    fn apply_adaptive(&mut self) {
        if !self.adaptive_pending {
            return;
        }
        self.adaptive_pending = false;
        let files: Vec<&Entry> = self.entries.iter().filter(|e| !e.is_dir()).collect();
        let media = files
            .iter()
            .filter(|e| matches!(icons::category(e), icons::Category::Image | icons::Category::Video))
            .count();
        if files.len() >= 4 && media * 2 >= files.len() {
            self.view_mode = ViewMode::Grid;
            self.grid_style = GridStyle::Tiles;
        }
    }

    fn grouped(&self) -> bool {
        self.view_mode == ViewMode::Details && !self.rows.is_empty()
    }

    /// The Grid with group headers (`GridUnit`).
    fn grid_grouped(&self) -> bool {
        self.view_mode == ViewMode::Grid && !self.rows.is_empty()
    }

    /// Slices per row of tiles in the grouped Grid, and a slice's height (a group header's).
    fn grid_slices(&self) -> (usize, Pixels) {
        let rh = f32::from(self.grid_row_h());
        let m = (rh / 30.).round().max(2.) as usize;
        (m, px(rh / m as f32))
    }

    /// The grouped Grid's lines, made again when what they come from changed.
    fn grid_units_now(&mut self) -> std::rc::Rc<(Vec<GridUnit>, Vec<usize>)> {
        let key = (self.rows.len(), self.visible.len(), self.grid_cols.max(1), self.grid_slices().0);
        if key != self.grid_units_for || self.grid_units.0.is_empty() {
            self.grid_units = std::rc::Rc::new(grid_units(&self.rows, key.2, key.3));
            self.grid_units_for = key;
        }
        self.grid_units.clone()
    }

    /// The list line to scroll to for visible index `vi`: in the grouped Grid the first or the last slice of its row of tiles,
    /// whichever brings the whole row into view from where the list stands.
    fn grid_line_of(&mut self, vi: usize) -> usize {
        if !self.grid_grouped() {
            return vi / self.grid_cols.max(1);
        }
        let units = self.grid_units_now();
        let first = units.1.get(vi).copied().unwrap_or(0);
        let (m, unit) = self.grid_slices();
        let top = (-f32::from(self.scroll.0.borrow().base_handle.offset().y) / f32::from(unit)).max(0.) as usize;
        if first < top { first } else { first + m - 1 }
    }

    /// Up or down one row of tiles in the grouped Grid: the same column of the next (previous) row, across a group header, or that
    /// row's last tile when it is shorter.
    fn grid_vertical(&mut self, vi: usize, down: bool) -> usize {
        let units = self.grid_units_now();
        let m = self.grid_slices().0;
        let Some(&first) = units.1.get(vi) else { return vi };
        let GridUnit::Tiles { start, .. } = units.0[first] else { return vi };
        let col = vi - start;
        let mut u = first as isize;
        loop {
            u += if down { m as isize } else { -1 };
            match usize::try_from(u).ok().and_then(|u| units.0.get(u)) {
                None => return vi,
                Some(GridUnit::Header(..)) => {
                    if down {
                        u -= m as isize - 1;
                    }
                }
                Some(GridUnit::Tiles { start, len, .. }) => return start + col.min(len - 1),
            }
        }
    }

    /// Details-list row for a visible index (accounts for group headers).
    fn list_row(&self, vi: usize) -> usize {
        if self.grouped() { self.row_of_vi.get(vi).copied().unwrap_or(vi) } else { vi }
    }

    pub fn set_query(&mut self, q: String, cx: &mut Context<Self>) {
        if self.filter.query != q {
            self.filter.query = q;
            self.rebuild_visible();
            cx.notify();
        }
    }

    pub fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        let v = !self.filter.show_hidden;
        crate::settings::update(cx, |s| s.show_hidden = v);
    }

    pub fn set_sort_spec(&mut self, spec: SortSpec, cx: &mut Context<Self>) {
        // a list sorted by a property column follows a change of direction or folder order
        if self.props_sort_spec_changed(spec, cx) {
            // (the list keeps its order until the property sort has been made again)
            cx.notify();
            return;
        }
        self.props_drop_sort();
        self.sort = spec;
        // (a sort by tag or path keeps the list as it is until its ranks are made: sorted by name now
        // and by the ranks a moment later would be two sorts and a flicker)
        self.sortx_ensure(cx);
        if !self.sortx_must_wait() {
            self.rebuild_visible();
        }
        self.remember_layout(cx);
        cx.notify();
    }

    pub fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        // from a sort by a property column, a fixed column starts as a new sort (not a flip of the one under it)
        self.sort = if self.props.sort_active() { self.props_fixed_sort(key) } else { self.sort.toggled(key) };
        self.props_drop_sort();
        self.sortx_ensure(cx);
        if !self.sortx_must_wait() {
            self.rebuild_visible();
        }
        self.remember_layout(cx);
        cx.notify();
    }

    // ---------- selection ----------

    fn select_only(&mut self, vi: usize) {
        self.selected.clear();
        if let Some(&ei) = self.visible.get(vi) {
            self.selected.insert(ei);
            self.cursor = Some(vi);
            self.anchor = Some(vi);
        }
    }

    fn select_range(&mut self, from: usize, to: usize) {
        self.selected.clear();
        let (a, b) = if from <= to { (from, to) } else { (to, from) };
        for vi in a..=b.min(self.visible.len().saturating_sub(1)) {
            self.selected.insert(self.visible[vi]);
        }
        self.cursor = Some(to);
    }

    /// The keyboard came to the list with Tab (issue 38): the item it is on (else the first selected) is brought into view; with
    /// nothing selected the first item is selected, so the move of the keyboard can be seen.
    pub(crate) fn keyboard_entered(&mut self, cx: &mut Context<Self>) {
        if self.visible.is_empty() || self.view_mode == ViewMode::Columns {
            return;
        }
        let vi = self.cursor.filter(|&c| c < self.visible.len()).or_else(|| self.visible.iter().position(|ei| self.selected.contains(ei)));
        let vi = match vi {
            Some(vi) => vi,
            None => {
                self.select_only(0);
                0
            }
        };
        self.cursor = Some(vi);
        let row = if self.view_mode == ViewMode::Grid { self.grid_line_of(vi) } else { self.list_row(vi) };
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// The selection moves to the next or the previous item (DirectLook's arrow keys, see `workspace::step_quick_look`).
    pub(crate) fn step_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.move_cursor(if forward { 1 } else { -1 }, false, cx);
    }

    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        let next = match self.cursor {
            None => {
                if delta >= 0 {
                    0
                } else {
                    last
                }
            }
            // (in the grouped Grid a step of a row's width is a step to the row below or above, across group headers)
            Some(c) if self.grid_grouped() && self.grid_cols > 1 && delta.unsigned_abs() == self.grid_cols => self.grid_vertical(c, delta > 0) as isize,
            Some(c) => (c as isize + delta).clamp(0, last),
        } as usize;
        if extend {
            let anchor = self.anchor.unwrap_or(next);
            self.anchor = Some(anchor);
            self.select_range(anchor, next);
        } else {
            self.select_only(next);
        }
        if self.list_rows > 0 {
            self.reveal_list_column(next);
        } else {
            let row = if self.view_mode == ViewMode::Grid { self.grid_line_of(next) } else { self.list_row(next) };
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn grid_tile_w(&self) -> Pixels {
        match self.grid_style {
            GridStyle::Tiles => px(GRID_SIZES[self.grid_size as usize].0),
            GridStyle::Cards => px(300.),
            GridStyle::List => px(240.),
        }
    }
    fn grid_row_h(&self) -> Pixels {
        match self.grid_style {
            GridStyle::Tiles => px(GRID_SIZES[self.grid_size as usize].1),
            GridStyle::Cards => px(84.),
            GridStyle::List => px(32.),
        }
    }
    fn grid_thumb_px(&self) -> u32 {
        match self.grid_style {
            GridStyle::Tiles => GRID_SIZES[self.grid_size as usize].2,
            GridStyle::Cards => 96,
            GridStyle::List => 32,
        }
    }

    /// The details list's empty strip on its right, for this list's width (see [`details_gutter`]).
    pub(crate) fn gutter(&self) -> f32 {
        details_gutter(f32::from(self.content_width))
    }

    pub fn row_h(&self) -> Pixels {
        px(ROW_SIZES[(self.row_size as usize).min(ROW_SIZES.len() - 1)])
    }

    /// The "Size" slider's position for the current layout (`None`: it has no sizes).
    pub fn layout_size(&self) -> Option<u8> {
        match (self.view_mode, self.grid_style) {
            (ViewMode::Details, _) => Some(self.row_size),
            (ViewMode::Grid, GridStyle::Tiles) => Some(self.grid_size),
            _ => None,
        }
    }

    pub fn set_layout_size(&mut self, n: u8, cx: &mut Context<Self>) {
        match (self.view_mode, self.grid_style) {
            (ViewMode::Details, _) => self.row_size = n.min(ROW_SIZES.len() as u8 - 1),
            (ViewMode::Grid, GridStyle::Tiles) => self.grid_size = n.min(GRID_SIZES.len() as u8 - 1),
            _ => return,
        }
        self.remember_layout(cx);
        cx.notify();
    }

    /// One size step bigger (`delta` > 0) or smaller, as Explorer's Ctrl+wheel goes: from the details list up into the grid at its
    /// smallest, through the grid's sizes, and from the smallest grid down into the details list again.
    pub fn zoom(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.view_mode != ViewMode::Grid || self.grid_style != GridStyle::Tiles {
            if delta > 0 {
                self.grid_size = 0;
                self.set_view_mode(ViewMode::Grid, cx);
            }
            return;
        }
        if delta < 0 && self.grid_size == 0 {
            self.set_view_mode(ViewMode::Details, cx);
            return;
        }
        let n = (self.grid_size as i32 + delta).clamp(0, GRID_SIZES.len() as i32 - 1) as u8;
        if n != self.grid_size {
            self.grid_size = n;
            self.remember_layout(cx);
            cx.notify();
        }
    }

    fn row_step(&self) -> isize {
        if self.view_mode == ViewMode::Grid && self.list_rows == 0 { self.grid_cols.max(1) as isize } else { 1 }
    }

    /// A step sideways: an item in the grid, a column of the List filled column by column.
    fn side_step(&self) -> isize {
        if self.list_rows > 0 { self.list_rows as isize } else { 1 }
    }

    /// Whether the folder shows the List layout filled column by column (Settings: `list_columns_first`; issue 170: "an optional
    /// Explorer-style List view that fills each column from top to bottom before starting the next column to the right").
    fn column_list(&self, cx: &App) -> bool {
        self.view_mode == ViewMode::Grid
            && self.grid_style == GridStyle::List
            && !self.grid_grouped()
            && cx.global::<crate::settings::Settings>().list_columns_first
    }

    /// The List filled column by column, as Explorer's List is: each column as many rows as the view is tall, the columns side by
    /// side and scrolled sideways (the wheel too). Only the columns in view are built. `before`: the rows per column of the last
    /// frame (the view's height is known from the frame before: a change draws once more).
    fn render_column_list(&mut self, before: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let count = self.visible.len();
        let base = self.scroll.0.borrow().base_handle.clone();
        let view = base.bounds();
        let row_h = self.grid_row_h();
        let view_h = if view.size.height > px(1.) { f32::from(view.size.height) } else { 480. };
        let rows = (((view_h - LIST_PAD_TOP * 2.) / f32::from(row_h)).floor() as usize).max(1);
        self.list_rows = rows;
        self.grid_cols = 1;
        if rows != before || view.size.height <= px(1.) {
            window.request_animation_frame();
        }
        let show_ext = cx.global::<crate::settings::Settings>().show_extensions;
        let widths = self.list_column_widths(rows, show_ext, window);
        let mut x = LIST_PAD_LEFT;
        self.list_cols = widths
            .iter()
            .map(|&w| {
                let c = (x, w);
                x += w + f32::from(GRID_GAP);
                c
            })
            .collect();
        let total = x + LIST_PAD_LEFT;
        let off = -f32::from(base.offset().x);
        let width = if view.size.width > px(1.) { f32::from(view.size.width) } else { f32::from(self.content_width) };
        let first = self.list_cols.partition_point(|&(x, w)| x + w < off);
        let last = self.list_cols.partition_point(|&(x, _)| x <= off + width);
        let mut g = GridCellCtx {
            hover_select: cx.global::<crate::settings::Settings>().select_on_hover,
            p: Palette::for_window(window),
            tile_w: px(LIST_COL_MIN),
            row_h,
            thumb_px: self.grid_thumb_px(),
            style: GridStyle::List,
            visual_px: px(20.),
            show_ext,
            renamed: self.renamed.clone(),
        };
        let mut columns = Vec::with_capacity(last.saturating_sub(first));
        for c in first..last {
            let (cx0, cw) = self.list_cols[c];
            g.tile_w = px(cw);
            let mut col = v_flex().absolute().top(px(LIST_PAD_TOP)).left(px(cx0)).w(px(cw));
            for vi in c * rows..((c + 1) * rows).min(count) {
                if let Some(cell) = self.grid_cell(vi, None, &g, window, cx) {
                    col = col.child(div().h(row_h).child(cell));
                }
            }
            columns.push(col);
        }
        div()
            .id("list-columns")
            .flex_1()
            .min_h_0()
            .overflow_x_scroll()
            .track_scroll(&base)
            // the wheel's turn up and down moves the columns sideways, as in Explorer's List
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                let d = ev.delta.pixel_delta(window.line_height());
                if d.x.abs() > px(0.5) || d.y.abs() < px(0.5) {
                    return;
                }
                let base = this.scroll.0.borrow().base_handle.clone();
                let most = base.max_offset().x;
                let mut o = base.offset();
                o.x = (o.x + d.y).clamp(-most, px(0.));
                base.set_offset(o);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().relative().h_full().w(px(total)).children(columns))
            .into_any_element()
    }

    /// The width of each column of the List filled column by column: as wide as its longest name (with the icon and the padding),
    /// within limits, as Explorer's List is (issue 170: "each column could size to its contents"). A folder of more than
    /// `LIST_MEASURE_MOST` items has columns of one width: measuring every name would hold the window up. The widths are kept for
    /// the same names in the same order, rows per column, extension setting and text size.
    fn list_column_widths(&mut self, rows: usize, show_ext: bool, window: &mut Window) -> Vec<f32> {
        let count = self.visible.len();
        let cols = count.div_ceil(rows.max(1));
        if count > LIST_MEASURE_MOST {
            return vec![f32::from(self.grid_tile_w()); cols];
        }
        let font_px = rems(0.875).to_pixels(window.rem_size());
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (self.entries_gen, rows, show_ext, f32::from(font_px).to_bits(), &self.visible).hash(&mut h);
            self.renamed.as_ref().map(|(a, b)| (a.to_string(), b.to_string())).hash(&mut h);
            h.finish()
        };
        if let Some((k, w)) = &self.list_widths
            && *k == key
        {
            return w.clone();
        }
        let style = window.text_style();
        let mut widths = Vec::with_capacity(cols);
        for c in 0..cols {
            let mut most = 0f32;
            for vi in c * rows..((c + 1) * rows).min(count) {
                let Some(&ei) = self.visible.get(vi) else { break };
                let name = row_name(&self.entries[ei as usize], show_ext, self.renamed.as_ref());
                let w = window.text_system().layout_line(&name, font_px, &[style.to_run(name.len())], None).width;
                most = most.max(f32::from(w));
            }
            // icon 20, the gaps and padding around it and the name, and a little air
            widths.push((most + LIST_CELL_CHROME).clamp(LIST_COL_MIN, LIST_COL_MAX));
        }
        self.list_widths = Some((key, widths.clone()));
        widths
    }

    /// The column of the List filled column by column under content x `x` (the nearest one).
    fn list_col_at(&self, x: f32) -> usize {
        self.list_cols.partition_point(|&(cx0, w)| cx0 + w < x).min(self.list_cols.len().saturating_sub(1))
    }

    /// Scrolls the List filled column by column so that the column of the visible index `vi` is in view.
    fn reveal_list_column(&mut self, vi: usize) {
        let rows = self.list_rows.max(1);
        let base = self.scroll.0.borrow().base_handle.clone();
        let width = f32::from(base.bounds().size.width);
        let Some(&(left, cw)) = self.list_cols.get(vi / rows) else { return };
        let right = left + cw + f32::from(GRID_GAP) + LIST_PAD_LEFT;
        let mut o = base.offset();
        let off = -f32::from(o.x);
        if left - LIST_PAD_LEFT < off {
            o.x = px(-(left - LIST_PAD_LEFT).max(0.));
        } else if width > 1. && right > off + width {
            o.x = px(-(right - width));
        } else {
            return;
        }
        base.set_offset(o);
    }

    pub fn set_view_mode(&mut self, m: ViewMode, cx: &mut Context<Self>) {
        self.set_layout(m, GridStyle::Tiles, cx)
    }

    /// View + grid look (Cards and List are grid styles).
    pub fn set_layout(&mut self, m: ViewMode, style: GridStyle, cx: &mut Context<Self>) {
        let style = if m == ViewMode::Grid { style } else { self.grid_style };
        if self.view_mode != m || self.grid_style != style {
            self.view_mode = m;
            self.grid_style = style;
            // (folding is the details list's: every item is back in the other views)
            if !self.collapsed.is_empty() {
                self.collapsed.clear();
                self.rebuild_visible();
            }
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.remember_layout(cx);
            cx.notify();
        }
    }

    fn render_grid_rows(&mut self, range: std::ops::Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let hover_select = cx.global::<crate::settings::Settings>().select_on_hover;
        let p = Palette::for_window(window);
        let cols = self.grid_cols.max(1);
        let (tile_w, row_h, thumb_px) = (self.grid_tile_w(), self.grid_row_h(), self.grid_thumb_px());
        let style = self.grid_style;
        let visual_px = match style {
            GridStyle::Tiles => px(thumb_px as f32 * 0.75),
            GridStyle::Cards => px(60.),
            GridStyle::List => px(20.),
        };
        let show_ext = cx.global::<crate::settings::Settings>().show_extensions;
        let renamed = self.renamed.clone();
        let mut out = Vec::with_capacity(range.len());
        let units = self.grid_grouped().then(|| self.grid_units_now());
        let (_, slice_h) = self.grid_slices();
        let g = GridCellCtx { hover_select, p, tile_w, row_h, thumb_px, style, visual_px, show_ext, renamed };
        for row in range {
            // (start, how many, the slice of the row) in the grouped Grid; a whole row of `cols` otherwise
            let (start, len, part) = match units.as_ref().map(|u| u.0.get(row).cloned()) {
                None => (row * cols, cols, None),
                Some(Some(GridUnit::Tiles { start, len, part })) => (start, len, Some(part)),
                Some(Some(GridUnit::Header(label, n))) => {
                    let folded = self.collapsed.contains(label.as_ref());
                    let key = label.to_string();
                    out.push(
                        h_flex()
                            .id(("ggrp", row))
                            .w_full()
                            .h(slice_h)
                            .px_2()
                            .gap_2()
                            .items_end()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_secondary)
                            .cursor_pointer()
                            .hover(|s| s.text_color(p.text))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&key, cx)))
                            .child(Icon::new(if folded { L::ChevronRight } else { L::ChevronDown }).size(px(12.)))
                            .child(crate::i18n::td(&label))
                            .child(div().text_color(p.text_tertiary).child(format!("{n}")))
                            .child(div().flex_1().mb(px(6.)).h(px(1.)).bg(p.divider))
                            .into_any_element(),
                    );
                    continue;
                }
                Some(None) => break,
            };
            let mut r = h_flex().w_full().h(self.grid_row_h()).gap(GRID_GAP).items_start();
            for c in 0..len {
                let Some(cell) = self.grid_cell(start + c, part, &g, window, cx) else { break };
                r = r.child(cell);
            }
            match part {
                None => out.push(r.into_any_element()),
                // one slice of the row: the row drawn whole, moved up by the slices above this one and cut to this slice
                Some(part) => out.push(
                    div()
                        .id(("gslice", row))
                        .w_full()
                        .h(slice_h)
                        .overflow_hidden()
                        .child(div().relative().top(-(slice_h * part as f32)).w_full().child(r))
                        .into_any_element(),
                ),
            }
        }
        out
    }

    /// One tile of the grid, in any of its looks, for the visible index `vi` (`part`: the slice of its row in the grouped Grid).
    fn grid_cell(&mut self, vi: usize, part: Option<usize>, g: &GridCellCtx, window: &mut Window, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let GridCellCtx { hover_select, p, tile_w, row_h, thumb_px, style, visual_px, show_ext, .. } = *g;
        let renamed = &g.renamed;
        let &ei = self.visible.get(vi)?;
        let e = &self.entries[ei as usize];
        let selected = self.selected.contains(&ei);
        let cat = icons::category(e);
        let dim = e.is_hidden() || e.is_cloud_placeholder();
        // Settings → "Show thumbnails" off: the file type's icon instead of a preview
        let thumbs_on = cx.global::<crate::settings::Settings>().show_thumbnails;
        let thumb = self.parent_of(ei).map(|d| d.to_path_buf()).and_then(|d| {
            self.images.update(cx, |c, cx| if thumbs_on { c.thumbnail(&d, e, thumb_px, cx) } else { c.icon(&d, e, cx) })
        });
        let is_thumb = icons::has_thumbnail(e);
        let visual: AnyElement = match thumb {
            Some(im) => img(im)
                .size(visual_px)
                .object_fit(ObjectFit::Contain)
                .when(is_thumb, |i| i.rounded(px(4.)))
                .into_any_element(),
            None => Icon::new(icons::icon_for(cat))
                .size(visual_px * 0.6)
                .text_color(theme::kind_color(cat, p.dark))
                .into_any_element(),
        };
        let overlay = self.overlay_of(ei, visual_px, cx);
        let e = &self.entries[ei as usize];
        let visual = download_marked(link_marked(visual, visual_px, e.link, overlay), self.part_frac(ei));
        let rename_input = self.renaming.as_ref().filter(|(i, _)| *i == ei).map(|(_, inp)| inp.clone());
        let label: AnyElement = match rename_input {
            // the text stays where the label was: centered, the label's size
            Some(inp) => div()
                .capture_key_down({
                    let input = inp.clone();
                    cx.listener(move |_, ev: &KeyDownEvent, _, cx| crate::rtl::handle_key_down(&input, ev, cx))
                })
                .w_full()
                .h(px(24.))
                // the label's line is shorter than the box: center the box on it
                .mt(px(-4.))
                .rounded(px(4.))
                .bg(p.card_bg)
                .border_1()
                .border_color(p.accent)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                // a press anywhere else applies the new name, as in Explorer (issue 180)
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.commit_rename_if_open(window, cx)))
                .child(gpui_kit::component::input::Input::new(&inp).appearance(false).small().px_1().text_xs().text_center())
                .into_any_element(),
            // a right-to-left name is split into its two lines here (issue 115: wrapped by GPUI, a long Persian name came out one
            // letter per line)
            None if crate::rtl::has_rtl(&e.name) => div()
                .w_full()
                .text_xs()
                .text_center()
                .when(dim, |d| d.opacity(0.6))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.name_hit = true))
                .children(rtl_grid_lines(&row_name(e, show_ext, renamed.as_ref()), tile_w - px(12.), window).into_iter().map(|line| {
                    div().w_full().whitespace_nowrap().overflow_hidden().text_ellipsis().child(crate::rtl::display_text(&line))
                }))
                .into_any_element(),
            None => div()
                .w_full()
                .text_xs()
                .text_center()
                .line_clamp(2)
                // a name longer than two lines ends in "…": without this the second line holds everything that is left and is
                // cut at both ends by the tile ("ding Page HTML Tem"), which reads as a broken name (a user's report)
                .text_ellipsis()
                .when(dim, |d| d.opacity(0.6))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.name_hit = true))
                .child(crate::rtl::display_text(&row_name(e, show_ext, renamed.as_ref())))
                .into_any_element(),
        };
        let facts = (style == GridStyle::Cards).then(|| {
            let mut f = vec![icons::type_label(e).to_string()];
            if !e.is_dir() {
                f.push(format::size(e.size));
            }
            f.push(format::datetime(e.modified_ms));
            f
        });
        let name_text = crate::rtl::display_text(&row_name(e, show_ext, renamed.as_ref()));
        let base = match style {
            GridStyle::Tiles => v_flex().p_1().gap_1().items_center().child(div().size(visual_px + px(4.)).flex().items_center().justify_center().child(visual)).child(label),
            GridStyle::Cards => h_flex()
                .px_2()
                .gap_3()
                .items_center()
                .child(div().size(visual_px + px(4.)).flex_shrink_0().flex().items_center().justify_center().child(visual))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).whitespace_nowrap().overflow_hidden().text_ellipsis().when(dim, |d| d.opacity(0.6)).on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.name_hit = true)).child(name_text.clone()))
                        .children(facts.unwrap_or_default().into_iter().map(|t| div().text_xs().text_color(p.text_secondary).whitespace_nowrap().overflow_hidden().text_ellipsis().child(t))),
                ),
            GridStyle::List => h_flex()
                .px_2()
                .gap_2()
                .items_center()
                .child(div().size(visual_px).flex_shrink_0().flex().items_center().justify_center().child(visual))
                .child({
                    // a name too long for the tile is cut before its extension, which stays in view (issue 170: "some names are cut
                    // off before I can see the .safetensors extension")
                    let shown = row_name(e, show_ext, renamed.as_ref());
                    let (stem, ext) = split_for_ellipsis(&shown, e.is_dir());
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .whitespace_nowrap()
                        .when(dim, |d| d.opacity(0.6))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.name_hit = true))
                        .child(div().min_w_0().overflow_hidden().text_ellipsis().child(crate::rtl::display_text(stem)))
                        .when(!ext.is_empty(), |d| d.child(div().flex_shrink_0().child(ext.to_string())))
                }),
        };
        let renaming_here = self.renaming.as_ref().is_some_and(|(i, _)| *i == ei);
        let base = if style != GridStyle::Tiles && renaming_here {
            // the rename box replaces the name line
            let inp = self.renaming.as_ref().map(|(_, i)| i.clone()).unwrap();
            h_flex().px_2().gap_2().items_center().child(div().size(visual_px).flex_shrink_0()).child(
                // the box grows to the left (border + padding) so the text keeps its place
                div()
                    .flex_1()
                    .h(px(24.))
                    .ml(px(-RENAME_INSET))
                    .px(px(RENAME_INSET - 1.))
                    .rounded(px(4.))
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.accent)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // a press anywhere else applies the new name, as in Explorer (issue 180)
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.commit_rename_if_open(window, cx)))
                    .child(gpui_kit::component::input::Input::new(&inp).appearance(false).small().px_0()),
            )
        } else {
            base
        };
        let cell = base
            .id(("cell", vi))
            .w(tile_w)
            .h(row_h - px(4.))
            .rounded(theme::RADIUS)
            // tiles: quiet fill + a thin accent outline (a pill would read as a crop)
            .when(selected, |d| d.bg(p.selection).border_1().border_color(p.selection_border))
            .when(!selected, |d| d.border_1().border_color(transparent_black()))
            .when(!selected && part.is_none(), |d| d.hover(|s| s.bg(p.hover)))
            .when(!selected && part.is_some() && self.grid_hover.is_some_and(|(h, _)| h == vi), |d| d.bg(p.hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.on_row_mouse_down(vi, ev, window, cx)),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, ev: &MouseUpEvent, _, cx| this.on_row_mouse_up(vi, ev, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| this.on_row_right_down(vi, window, cx)),
            )
            .when_some(self.path_of(ei), |d, path| file_drop_target(d, path, e.is_dir(), &p))
            // Settings → "Select files and folders on hover"; in a sliced row also the tile's fill (`grid_hover`)
            .when(hover_select || part.is_some(), |d| {
                let sliced = part.is_some();
                d.on_hover(cx.listener(move |this, on: &bool, _, cx| this.cell_hover(vi, *on, sliced, hover_select, cx)))
            });
        Some(cell)
    }

    fn on_row_mouse_down(&mut self, vi: usize, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        // (a press on an item of a window that is not in front leaves it there until the press is not a drag: issue 169)
        rf_win::press_activate::press_on_item();
        // before anything changes: was this the only selected item, in a focused, active list?
        let on_name = std::mem::take(&mut self.name_hit);
        let was_renaming = self.renaming.is_some();
        let had_focus = window.is_window_active() && self.focus.is_focused(window);
        let was_only = self.visible.get(vi).is_some_and(|ei| self.selected.len() == 1 && self.selected.contains(ei));
        self.slow_click = None;
        // a press on another item applies the name being typed, as Explorer does (issue 180: "With Windows Explorer you could just
        // click anywhere on the window for the changes to be applied"); it was thrown away
        self.commit_rename_if_open(window, cx);
        window.focus(&self.focus, cx);
        let Some(&ei) = self.visible.get(vi) else { return };
        let plain = !ev.modifiers.control && !ev.modifiers.shift;
        // Settings → "Open items with a single click"
        let single = self.opens_on_single_click(ei, cx);
        // A second click opens only what the first click of the pair was made on (`completes_double`). Otherwise it is a click like
        // any other: it selects.
        let name = self.entries[ei as usize].name.clone();
        let double = ev.click_count >= 2 && completes_double(self.first_click.as_ref(), self.load_serial, Some(&name));
        self.first_click = (ev.click_count == 1).then(|| FirstClick { load: self.load_serial, name: Some(name) });
        if ev.click_count >= 2 && !double {
            // (it landed on this row because the listing changed under the pointer: the user did not aim at it, so it does not even
            // select it)
            return;
        }
        if plain && double {
            // with "open with a single click" the first release has opened it already: a double click out of habit must not start
            // a program twice
            if !single {
                self.select_only(vi);
                self.open_entry(ei, cx);
            }
            return;
        }
        if ev.modifiers.shift {
            let anchor = self.anchor.unwrap_or(vi);
            self.select_range(anchor, vi);
            self.anchor = Some(anchor);
        } else if ev.modifiers.control {
            if !self.selected.remove(&ei) {
                self.selected.insert(ei);
            }
            self.cursor = Some(vi);
            self.anchor = Some(vi);
        } else if !self.selected.contains(&ei) {
            self.select_only(vi);
        } else {
            // Clicking inside an existing multi-selection keeps it (so it can be dragged).
            self.cursor = Some(vi);
        }
        self.drag_origin = Some(RowPress { origin: ev.position, open_on_release: plain && single && ev.click_count == 1 });
        // Explorer: a click on the name of the item that was already the only one selected
        // renames it, once the double-click time passes without a second click or a drag
        if plain && !single && ev.click_count == 1 && on_name && was_only && had_focus && !was_renaming {
            self.arm_slow_click(ei, cx);
        }
        cx.notify();
    }

    fn opens_on_single_click(&self, ei: u32, cx: &App) -> bool {
        match cx.global::<crate::settings::Settings>().single_click {
            crate::settings::ClickPref::Never => false,
            crate::settings::ClickPref::Folders => self.entries[ei as usize].is_dir(),
            crate::settings::ClickPref::Always => true,
        }
    }

    fn on_row_mouse_up(&mut self, vi: usize, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(&ei) = self.visible.get(vi) else { return };
        // Open on release so a single-click setting still allows the item to start a drag.
        if self.drag_origin.as_ref().is_some_and(|press| press.open_on_release)
            && !ev.modifiers.control && !ev.modifiers.shift
            && completes_double(self.first_click.as_ref(), self.load_serial, Some(&self.entries[ei as usize].name))
        {
            self.drag_origin = None;
            self.select_only(vi);
            self.open_entry(ei, cx);
        }
    }

    /// Settings → "Select on hover": the pointer came onto (or left) the item at `vi`. Like Explorer's "point to select" the item is
    /// selected once the pointer has rested on it for the system's hover time, so passing over items on the way to another one
    /// leaves the selection alone (issue 101: it was in the Details layout only, and at once).
    /// The pointer came onto (or left) a tile of the Grid. In a sliced row (the grouped Grid) it comes onto one slice's copy of the tile
    /// after another: the tile counts as hovered while the pointer is over any of them (`grid_hover`), whatever order the copies are told
    /// in, and "select on hover" hears only the tile's own coming and going.
    fn cell_hover(&mut self, vi: usize, on: bool, sliced: bool, hover_select: bool, cx: &mut Context<Self>) {
        if !sliced {
            if hover_select {
                self.hover_row(vi, on, cx);
            }
            return;
        }
        let before = self.grid_hover.map(|h| h.0);
        self.grid_hover = grid_hover_step(self.grid_hover, vi, on);
        let after = self.grid_hover.map(|h| h.0);
        if before != after {
            if hover_select {
                if let Some(b) = before {
                    self.hover_row(b, false, cx);
                }
                if let Some(a) = after {
                    self.hover_row(a, true, cx);
                }
            }
            cx.notify();
        }
    }

    fn hover_row(&mut self, vi: usize, on: bool, cx: &mut Context<Self>) {
        let Some(&ei) = self.visible.get(vi) else { return };
        if !on {
            if self.hover_pick.is_some_and(|(e, _)| e == ei) {
                self.hover_pick = None;
            }
            return;
        }
        self.slow_click_serial += 1;
        let serial = self.slow_click_serial;
        self.hover_pick = Some((ei, serial));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(hover_time()).await;
            let _ = this.update(cx, |v, cx| {
                if v.hover_pick != Some((ei, serial)) {
                    return;
                }
                v.hover_pick = None;
                // not while a press, a drag, a rubber band or a rename is going on, and not over a multi-selection made by hand
                let busy = v.drag_origin.is_some() || v.marquee.is_some() || v.renaming.is_some();
                let Some(now) = v.visible.iter().position(|&e| e == ei) else { return };
                if busy {
                    return;
                }
                // with Ctrl held the item joins the selection, with Shift the selection runs from the anchor to it, as in Explorer's
                // "point to select" (issue 101: each hover replaced the selection)
                let (ctrl, shift) = unsafe {
                    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
                    (GetKeyState(VK_CONTROL.0 as i32) < 0, GetKeyState(VK_SHIFT.0 as i32) < 0)
                };
                if shift {
                    let anchor = v.anchor.unwrap_or(now);
                    v.select_range(anchor, now);
                    v.anchor = Some(anchor);
                    cx.notify();
                } else if ctrl {
                    if v.selected.insert(ei) {
                        v.cursor = Some(now);
                        v.anchor = Some(now);
                        cx.notify();
                    }
                } else if !(v.selected.len() == 1 && v.selected.contains(&ei)) {
                    v.select_only(now);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A click on a file in a column to the right of the list's: once the double-click time passes without a second click, that
    /// column's folder becomes the list's and the file is selected in it. Not at once: the columns move over by one then, and the
    /// second click of a double click (which opens the file) has to land on the same row first.
    fn pick_in_column(&mut self, dir: PathBuf, name: String, cx: &mut Context<Self>) {
        self.static_pick += 1;
        let serial = self.static_pick;
        let wait = std::time::Duration::from_millis(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as u64);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                if v.static_pick == serial {
                    v.set_reselect(name);
                    v.navigate(Location::Path(dir), cx);
                }
            });
        })
        .detach();
    }

    fn arm_slow_click(&mut self, ei: u32, cx: &mut Context<Self>) {
        self.slow_click_serial += 1;
        let serial = self.slow_click_serial;
        self.slow_click = Some((ei, serial));
        let wait = std::time::Duration::from_millis(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as u64);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                if v.slow_click != Some((ei, serial)) {
                    return;
                }
                v.slow_click = None;
                // the selection must still be just this item, under the cursor
                let still = v.selected.len() == 1 && v.selected.contains(&ei) && v.cursor_entry() == Some(ei);
                if still && v.renaming.is_none() {
                    v.request_rename();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_drag_motion(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = &self.drag_origin else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag_origin = None;
            return;
        }
        let d = ev.position - press.origin;
        if d.x.abs() + d.y.abs() > px(8.) {
            self.drag_origin = None;
            self.slow_click = None;
            self.start_drag_out(window, cx);
        }
    }

    /// Window position → coordinates inside the scrolled list content.
    fn content_point(&self, pos: Point<Pixels>) -> Point<Pixels> {
        let st = self.scroll.0.borrow();
        let b = st.base_handle.bounds();
        let off = st.base_handle.offset();
        point(pos.x - b.origin.x - off.x, pos.y - b.origin.y - off.y)
    }

    fn begin_marquee(&mut self, ev: &MouseDownEvent) {
        let p = self.content_point(ev.position);
        let base = if ev.modifiers.control || ev.modifiers.shift { self.selected.clone() } else { HashSet::new() };
        self.marquee = Some(Marquee { start: p, cur: p, base });
    }

    fn update_marquee(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let cur = self.content_point(pos);
        let Some(m) = self.marquee.as_mut() else { return };
        m.cur = cur;
        let (x0, x1) = (f32::from(m.start.x.min(cur.x)), f32::from(m.start.x.max(cur.x)));
        let (y0, y1) = (f32::from(m.start.y.min(cur.y)), f32::from(m.start.y.max(cur.y)));
        let mut sel = m.base.clone();
        let n = self.visible.len();
        if n > 0 && (y1 - y0 > 2.0 || x1 - x0 > 2.0) {
            match self.view_mode {
                ViewMode::Details => {
                    let pad = 4.0;
                    let h = f32::from(self.row_h());
                    if y1 >= pad {
                        let rows = if self.grouped() { self.rows.len() } else { n };
                        let first = ((y0 - pad).max(0.0) / h) as usize;
                        let last = (((y1 - pad) / h) as usize).min(rows - 1);
                        for r in first..=last.max(first).min(rows - 1) {
                            let vi = if self.grouped() {
                                match self.rows[r] {
                                    Row::Item(vi) => vi,
                                    Row::Header(..) => continue,
                                }
                            } else {
                                r
                            };
                            if let Some(&ei) = self.visible.get(vi) {
                                sel.insert(ei);
                            }
                        }
                    }
                }
                ViewMode::Columns => {}
                ViewMode::Grid => {
                    let (pt, pl) = (8.0, 6.0);
                    let cw = f32::from(self.grid_tile_w() + GRID_GAP);
                    let rh = f32::from(self.grid_row_h());
                    let cols = self.grid_cols.max(1);
                    if self.list_rows > 0 {
                        // the List filled column by column: the rows and the columns the band touches
                        let rows = self.list_rows;
                        let (pt, pl) = (LIST_PAD_TOP, LIST_PAD_LEFT);
                        if y1 >= pt && x1 >= pl {
                            let r0 = ((y0 - pt).max(0.0) / rh) as usize;
                            let r1 = (((y1 - pt) / rh) as usize).min(rows - 1);
                            let (c0, c1) = (self.list_col_at(x0), self.list_col_at(x1));
                            for c in c0..=c1 {
                                for r in r0..=r1.max(r0) {
                                    if let Some(&ei) = self.visible.get(c * rows + r) {
                                        sel.insert(ei);
                                    }
                                }
                            }
                        }
                    } else if self.grid_grouped() && y1 >= pt && x1 >= pl {
                        // lines one slice high: every row of tiles a slice of the band touches
                        let units = self.grid_units_now();
                        let unit = f32::from(self.grid_slices().1);
                        let u0 = ((y0 - pt).max(0.0) / unit) as usize;
                        let u1 = (((y1 - pt) / unit) as usize).min(units.0.len().saturating_sub(1));
                        let c0 = ((x0 - pl).max(0.0) / cw) as usize;
                        let c1 = (((x1 - pl) / cw) as usize).min(cols - 1);
                        for u in u0..=u1 {
                            if let Some(GridUnit::Tiles { start, len, .. }) = units.0.get(u) {
                                for c in c0..=c1.min(len - 1) {
                                    if let Some(&ei) = self.visible.get(start + c) {
                                        sel.insert(ei);
                                    }
                                }
                            }
                        }
                    } else if y1 >= pt && x1 >= pl {
                        let r0 = ((y0 - pt).max(0.0) / rh) as usize;
                        let r1 = ((y1 - pt) / rh) as usize;
                        let c0 = ((x0 - pl).max(0.0) / cw) as usize;
                        let c1 = (((x1 - pl) / cw) as usize).min(cols - 1);
                        for r in r0..=r1 {
                            for c in c0..=c1 {
                                let vi = r * cols + c;
                                if vi < n {
                                    sel.insert(self.visible[vi]);
                                }
                            }
                        }
                    }
                }
            }
        }
        self.selected = sel;
        cx.notify();
    }

    fn render_marquee(&self, p: &Palette) -> Option<AnyElement> {
        let m = self.marquee.as_ref()?;
        let off = self.scroll.0.borrow().base_handle.offset();
        let x0 = m.start.x.min(m.cur.x) + off.x;
        let y0 = m.start.y.min(m.cur.y) + off.y;
        let w = (m.start.x - m.cur.x).abs();
        let h = (m.start.y - m.cur.y).abs();
        if w < px(3.) && h < px(3.) {
            return None;
        }
        Some(
            div()
                .absolute()
                .left(x0)
                .top(y0)
                .w(w)
                .h(h)
                .bg(p.selection)
                .border_1()
                .border_color(p.selection_border)
                .into_any_element(),
        )
    }

    /// Space held to peek (QuickLook's "hold to preview"): released after a long press, the
    /// preview it opened closes. A short press keeps toggling as before.
    fn on_key_up(&mut self, ev: &KeyUpEvent, cx: &mut Context<Self>) {
        if ev.keystroke.key != "space" {
            return;
        }
        let Some(pressed) = self.space_hold.take() else { return };
        if pressed.elapsed() >= std::time::Duration::from_millis(400)
            && QUICK_LOOK_OPEN.load(Ordering::Relaxed)
            && crate::look::config::LookSettings::load().look_hold_to_close
        {
            cx.emit(FolderEvent::QuickLook);
        }
    }

    /// Explorer-style type-ahead: typing selects the next item whose name starts with the typed text.
    fn on_typeahead(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        // Ctrl+Alt together is AltGr on Windows (Turkish Q: AltGr+ı = i, AltGr+Q = @): that is text
        let altgr = m.control && m.alt;
        if ((m.control || m.alt) && !altgr) || m.platform || self.renaming.is_some() {
            return;
        }
        let now = Instant::now();
        // Space: Quick Look — unless a type-ahead name is being typed ("my file")
        if ev.keystroke.key == "space" && !m.shift {
            let typing = !self.typeahead.0.is_empty() && now.duration_since(self.typeahead.1) <= std::time::Duration::from_millis(900);
            if !typing {
                // holding Space repeats the key: one toggle per press
                if ev.is_held {
                    cx.stop_propagation();
                    return;
                }
                if self.cursor_entry().is_some() || QUICK_LOOK_OPEN.load(Ordering::Relaxed) {
                    self.space_hold = (!QUICK_LOOK_OPEN.load(Ordering::Relaxed)).then_some(now);
                    cx.emit(FolderEvent::QuickLook);
                    cx.stop_propagation();
                }
                return;
            }
        }
        let Some(ch) = ev.keystroke.key_char.as_ref().filter(|c| !c.chars().any(|c| c.is_control())) else { return };
        // Settings → "Typing filters this folder" (OneCommander's way, issue 106): the letters go to the filter bar instead
        if !ch.is_empty() && cx.global::<crate::settings::Settings>().type_to_filter && self.location().path().is_some() && !self.picking {
            cx.emit(FolderEvent::StartFilter(ch.clone()));
            cx.stop_propagation();
            return;
        }
        if ch.is_empty() || self.visible.is_empty() {
            return;
        }
        let fresh = now.duration_since(self.typeahead.1) > std::time::Duration::from_millis(900);
        if fresh {
            self.typeahead.0.clear();
        }
        self.typeahead.0.push_str(&rf_core::entry::fold_case(ch));
        self.typeahead.1 = now;
        let needle = self.typeahead.0.clone();
        // Repeating one letter cycles through matches; otherwise search from the cursor.
        let single_repeat = needle.chars().count() > 1 && needle.chars().all(|c| Some(c) == needle.chars().next());
        let needle = if single_repeat { needle.chars().next().unwrap().to_string() } else { needle };
        let n = self.visible.len();
        let start = match self.cursor {
            Some(c) if fresh || single_repeat => c + 1,
            Some(c) => c,
            None => 0,
        };
        for k in 0..n {
            let vi = (start + k) % n;
            let name = rf_core::entry::fold_case(self.entries[self.visible[vi] as usize].shown());
            if name.starts_with(&needle) {
                self.select_only(vi);
                let row = if self.view_mode == ViewMode::Grid { self.grid_line_of(vi) } else { self.list_row(vi) };
                self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
    }

    pub fn selection_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<u32> = self.selected.iter().copied().collect();
        v.sort_unstable();
        v.into_iter().filter_map(|i| self.path_of(i)).collect()
    }

    /// The selection, each with whether it is a folder (no disk access).
    pub fn selection_with_kind(&self) -> Vec<(PathBuf, bool)> {
        let mut v: Vec<u32> = self.selected.iter().copied().collect();
        v.sort_unstable();
        v.into_iter().filter_map(|i| self.path_of(i).map(|p| (p, self.entries[i as usize].is_dir()))).collect()
    }

    pub fn item_count(&self) -> usize {
        self.visible.len()
    }

    pub fn selection_summary(&self) -> (usize, u64, bool) {
        let mut size = 0;
        let mut has_dir = false;
        for &i in &self.selected {
            let e = &self.entries[i as usize];
            size += e.size;
            has_dir |= e.is_dir();
        }
        (self.selected.len(), size, has_dir)
    }

    /// Folders whose size was measured carry it in their entry, so sorting by Size puts them
    /// among the files by their real size. Returns whether anything changed.
    fn apply_dir_sizes(&mut self, on_disk: bool) -> bool {
        if self.folder_sizes.is_empty() {
            return false;
        }
        let mut changed = false;
        for i in 0..self.entries.len() {
            if !self.entries[i].is_dir() {
                continue;
            }
            let Some(p) = self.path_of(i as u32) else { continue };
            if let Some(DirSize::Done(s)) = self.folder_sizes.get(&p) {
                let b = DirSize::bytes(*s, on_disk);
                if self.entries[i].size != b {
                    self.entries[i].size = b;
                    self.entries_gen += 1;
                    changed = true;
                }
            }
        }
        changed
    }

    /// Re-sorts when measured folder sizes changed what a Size sort shows.
    fn resort_for_sizes(&mut self, cx: &App) {
        let on_disk = cx.global::<crate::settings::Settings>().folder_size_on_disk;
        if self.apply_dir_sizes(on_disk) && self.sort.key == SortKey::Size {
            self.rebuild_visible();
        }
    }

    /// Narrow panes (dual pane, small windows) drop columns: Type first, then Date.
    /// Size of folder `p` as far as known; asks for it when it was never requested. Requests of
    /// one frame start together as one batch (all folders walk in parallel, each shows up as soon
    /// as it is done; running totals in between).
    fn folder_size(&mut self, p: PathBuf, cx: &mut Context<Self>) -> DirSize {
        if let Some(v) = self.folder_sizes.get(&p) {
            return *v;
        }
        self.folder_sizes.insert(p.clone(), DirSize::Partial(0));
        if self.size_queue.is_empty() {
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = this.update(cx, |v, cx| v.start_size_batch(cx));
            });
        }
        self.size_queue.push(p);
        DirSize::Partial(0)
    }

    fn start_size_batch(&mut self, cx: &mut Context<Self>) {
        let dirs = std::mem::take(&mut self.size_queue);
        if dirs.is_empty() {
            return;
        }
        self.size_batches.push((rf_win::foldersize::SizeBatch::start(dirs.clone()), dirs));
        if self.size_polling {
            return;
        }
        self.size_polling = true;
        cx.spawn(async move |this, cx| {
            // quick first look (small folders and cache hits are done within milliseconds),
            // then running totals ten times a second
            let mut wait = std::time::Duration::from_millis(15);
            loop {
                cx.background_executor().timer(wait).await;
                wait = std::time::Duration::from_millis(100);
                let more = this
                    .update(cx, |v, cx| {
                        let (mut changed, mut finished) = (false, false);
                        for (b, dirs) in &v.size_batches {
                            for (i, d) in dirs.iter().enumerate() {
                                let now = DirSize::from(b.state(i));
                                if v.folder_sizes.get(d) != Some(&now) {
                                    finished |= matches!(now, DirSize::Done(_));
                                    v.folder_sizes.insert(d.clone(), now);
                                    changed = true;
                                }
                            }
                        }
                        v.size_batches.retain(|(b, _)| !b.finished());
                        if changed {
                            // running totals only redraw; a finished size goes into the entries
                            // (a pass over every folder of the listing), not ten times a second
                            if finished {
                                v.resort_for_sizes(cx);
                            }
                            cx.notify();
                        }
                        let more = !v.size_batches.is_empty();
                        v.size_polling = more;
                        more
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    fn type_col_w(&self, cx: &App) -> Pixels {
        if self.is_search() {
            // location shares the free width with the name instead of squeezing it
            let free = f32::from(self.content_width) - 150. - 100. - 40. - self.gutter();
            px((free * 0.45).clamp(160., 460.))
        } else {
            px(self.col_widths(cx)[2])
        }
    }

    /// Results from many folders (search, tag): entries carry their own parent folder.
    pub fn is_search(&self) -> bool {
        matches!(self.location(), Location::Search { .. } | Location::Tag(_) | Location::Library { .. })
    }

    /// Folder containing entry `ei` (differs per entry in search results).
    pub(crate) fn parent_of(&self, ei: u32) -> Option<&std::path::Path> {
        if matches!(self.location(), Location::Shell { .. }) {
            return self.shell_items.get(ei as usize)?.fs_path.as_deref()?.parent();
        }
        match self.parents.get(ei as usize) {
            Some(p) => Some(p),
            None => self.location().path(),
        }
    }

    pub(crate) fn path_of(&self, ei: u32) -> Option<PathBuf> {
        if matches!(self.location(), Location::Shell { .. }) {
            // only items that are real files have a path; a phone's items have none
            return self.shell_items.get(ei as usize)?.fs_path.clone();
        }
        Some(self.parent_of(ei)?.join(&*self.entries.get(ei as usize)?.name))
    }

    pub(crate) fn places_pinned_paths(&self, cx: &App) -> Vec<PathBuf> {
        self.places.read(cx).pinned.iter().map(|p| p.path.clone()).collect()
    }

    /// The drives "Send to" lists after the SendTo folder's entries, as Explorer's does: the removable and the network ones (root,
    /// name, icon).
    pub(crate) fn send_to_drives(&self, cx: &App) -> Vec<(PathBuf, String, gpui_kit::assets::IconName)> {
        use rf_win::drives::DriveKind;
        self.places
            .read(cx)
            .drives
            .iter()
            .filter(|d| matches!(d.kind, DriveKind::Removable | DriveKind::Network))
            .map(|d| (d.root.clone(), places::drive_name(d), places::drive_icon(d)))
            .collect()
    }

    pub(crate) fn refresh_places(&self, cx: &mut Context<Self>) {
        self.places.update(cx, |p, cx| p.refresh(cx));
    }

    /// Item shown in the preview pane.
    pub fn preview_target(&self) -> Option<(PathBuf, Entry)> {
        let ei = self.cursor_entry()?;
        let e = self.entries.get(ei as usize)?;
        Some((self.path_of(ei)?, e.clone()))
    }

    pub(crate) fn entry(&self, ei: u32) -> &Entry {
        &self.entries[ei as usize]
    }

    /// Entry under the keyboard cursor, or the single selected entry.
    pub(crate) fn cursor_entry(&self) -> Option<u32> {
        self.cursor
            .and_then(|c| self.visible.get(c).copied())
            .filter(|ei| self.selected.contains(ei))
            .or_else(|| (self.selected.len() == 1).then(|| *self.selected.iter().next().unwrap()))
    }

    pub(crate) fn set_renaming(
        &mut self,
        r: Option<(u32, Entity<gpui_kit::component::input::InputState>)>,
        sub: Option<Subscription>,
    ) {
        self.renaming = r;
        self._rename_sub = sub;
    }

    /// Select these names (in the current folder) as soon as they're listed: a paste or a copy
    /// into this folder shows what arrived, like Explorer does.
    pub(crate) fn select_when_listed(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        if names.is_empty() {
            return;
        }
        self.reselect_many = Some((names, Instant::now() + std::time::Duration::from_secs(6)));
        self.soft_reload(cx);
    }

    pub(crate) fn set_reselect(&mut self, name: String) {
        self.reselect = Some(name);
    }

    /// Selects `name` and scrolls to it: now when the folder is listed, or once it is.
    pub(crate) fn select_now(&mut self, name: String, cx: &mut Context<Self>) {
        self.reselect = Some(name);
        if !self.loading {
            self.apply_reselect(cx);
        }
        cx.notify();
    }

    pub(crate) fn clear_reselect(&mut self) {
        self.reselect = None;
        self.rename_after_load = false;
        self.menu_after_load = None;
    }

    /// A right-click on an item in another column of the columns layout: that column becomes the list's, the item is selected and
    /// its menu shows. WHY (issue 183: "When right-clicking on the last column ... it closes and goes back to the previous
    /// column"): the press went through to the list's background, which took it as a click on nothing and cleared the selection,
    /// and the column of the selected folder went away.
    fn menu_in_column(&mut self, dir: PathBuf, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.static_pick += 1; // (a single click's pick still waiting is not wanted any more)
        self.menu_after_load = Some(window.window_handle());
        self.set_reselect(name);
        self.navigate(Location::Path(dir), cx);
    }

    /// A rename is being carried out: the row of `old` shows `new` until the listing has the new name (see `renamed`).
    pub(crate) fn show_renamed(&mut self, old: &str, new: &str) {
        self.renamed = Some((old.into(), new.into()));
    }

    /// A rename did not happen (the answer was no, the name was taken, the system refused): the row shows its own name again.
    pub(crate) fn rename_failed(&mut self, cx: &mut Context<Self>) {
        self.clear_reselect();
        if self.renamed.take().is_some() {
            cx.notify();
        }
    }

    /// Open/Save dialog mode: which files show (patterns like "*.txt;*.md", empty = all; none
    /// when picking a folder), and opening a file picks it.
    pub(crate) fn set_picking(&mut self, patterns: String, folders_only: bool, cx: &mut Context<Self>) {
        self.picking = true;
        if self.filter.patterns != patterns || self.filter.folders_only != folders_only {
            self.filter.patterns = patterns;
            self.filter.folders_only = folders_only;
            self.rebuild_visible();
            cx.notify();
        }
    }

    pub(crate) fn request_rename(&mut self) {
        self.pending_rename = true;
    }

    /// A new item was created; select it and start rename as soon as the watcher lists it.
    pub(crate) fn expect_new_item(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if let Some(n) = name {
            self.reselect = Some(n);
            self.rename_after_load = true;
            self.soft_reload(cx);
        }
    }

    fn on_row_right_down(&mut self, vi: usize, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        if let Some(&ei) = self.visible.get(vi) {
            if !self.selected.contains(&ei) {
                self.select_only(vi);
            }
        }
        cx.notify();
        self.show_context_menu(false, window, cx);
    }

    // ---------- opening ----------

    /// Shell folders (phones…): opens what only the shell can open. Returns false for items
    /// that are real files or folders (they go the ordinary way through `path_of`).
    fn open_shell_entry(&mut self, ei: u32, cx: &mut Context<Self>) -> bool {
        let Some(child) = self.shell_items.get(ei as usize).cloned() else { return true };
        let loc = if child.is_folder {
            match &child.fs_path {
                Some(p) if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("library-ms")) => {
                    Location::Library { file: p.clone(), name: child.name.clone() }
                }
                Some(_) => return false,
                // a network computer (`\nas`) is a path to us, the shell lists its shares
                None if child.parsing.starts_with(r"\\") => Location::Path(PathBuf::from(&child.parsing)),
                None => Location::Shell { parsing: child.parsing.clone(), name: child.name.clone() },
            }
        } else {
            if child.fs_path.is_some() {
                return false;
            }
            // an item on a phone: the shell fetches it and opens it with its default app
            // (a dialog asking for a file cannot be answered with one)
            if !self.picking {
                let parsing = child.parsing;
                std::thread::spawn(move || {
                    if let Err(e) = rf_win::shellfolder::open(&parsing) {
                        log::warn!("open {parsing}: {e:#}");
                    }
                });
            }
            return true;
        };
        if cx.global::<crate::settings::Settings>().open_folders_in_new_tab {
            cx.emit(FolderEvent::OpenInNewTab(loc));
        } else {
            self.navigate(loc, cx);
        }
        true
    }

    /// The selected items of a shell folder, in list order.
    pub(crate) fn selection_shell(&self) -> Vec<rf_win::shellfolder::ShellChild> {
        let mut v: Vec<u32> = self.selected.iter().copied().collect();
        v.sort_unstable();
        v.into_iter().filter_map(|i| self.shell_items.get(i as usize).cloned()).collect()
    }

    fn open_entry(&mut self, ei: u32, cx: &mut Context<Self>) {
        if matches!(self.location(), Location::Shell { .. }) && self.open_shell_entry(ei, cx) {
            return;
        }
        let Some(path) = self.path_of(ei) else { return };
        let e = &self.entries[ei as usize];
        let browse = !e.is_dir()
            && cx.global::<crate::settings::Settings>().browse_archives
            && crate::archive_view::is_browsable(&path);
        // an archive goes where the setting says (its own tab or window, or here as a folder)
        let archive_to = if e.is_dir() { crate::settings::ArchiveOpen::Here } else { cx.global::<crate::settings::Settings>().archives_open_in };
        if browse && archive_to == crate::settings::ArchiveOpen::NewWindow {
            cx.emit(FolderEvent::OpenInNewWindow(Location::Path(path)));
        } else if browse && archive_to == crate::settings::ArchiveOpen::NewTab {
            cx.emit(FolderEvent::OpenInNewTab(Location::Path(path)));
        } else if e.is_dir() || browse {
            if cx.global::<crate::settings::Settings>().open_folders_in_new_tab {
                cx.emit(FolderEvent::OpenInNewTab(Location::Path(path)));
            } else {
                self.navigate(Location::Path(path), cx);
            }
        } else if self.picking {
            cx.emit(FolderEvent::Picked(vec![path]));
        } else if is_lnk(&path) {
            self.open_shortcut(path, cx);
        } else {
            open_in_background_sorted(path, self.sort.key, self.sort.dir == SortDir::Asc, cx);
        }
    }

    /// A `.lnk` file: one that points at a folder opens that folder here, as a folder would (a user on Discord, 2026-11-20: it opened
    /// in Explorer); any other goes to the shell as before. The link is read off the UI thread (COM, and a target that has to wake up).
    fn open_shortcut(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let link = path.clone();
            let folder = cx
                .background_executor()
                .spawn(async move { rf_win::shortcut::resolve(&link).ok().map(|i| i.target).filter(|t| !t.as_os_str().is_empty() && t.is_dir()) })
                .await;
            let _ = this.update(cx, |this, cx| match folder {
                Some(dir) if cx.global::<crate::settings::Settings>().open_folders_in_new_tab => cx.emit(FolderEvent::OpenInNewTab(Location::Path(dir))),
                Some(dir) => this.navigate(Location::Path(dir), cx),
                None => open_in_background(path, cx),
            });
        })
        .detach();
    }

    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let sel: Vec<u32> = match self.cursor.and_then(|c| self.visible.get(c)).copied() {
            Some(c) if self.selected.len() <= 1 => vec![c],
            _ => self.selected.iter().copied().collect(),
        };
        if sel.len() == 1 {
            self.open_entry(sel[0], cx);
        } else if self.picking {
            let files: Vec<PathBuf> = sel.iter().filter(|&&i| !self.entries[i as usize].is_dir()).filter_map(|&i| self.path_of(i)).collect();
            if !files.is_empty() {
                cx.emit(FolderEvent::Picked(files));
            }
        } else {
            let mut in_archive = Vec::new();
            for i in sel {
                if !self.entries[i as usize].is_dir() {
                    if let Some(p) = self.path_of(i) {
                        if crate::nav_touch::may_hold_archive(&p) && !p.exists() && crate::archive_view::split(&p).is_some() {
                            // extracted together, one Status Center entry
                            in_archive.push(p);
                        } else {
                            open_in_background(p, cx);
                        }
                    }
                }
            }
            crate::archive_view::open_items(cx, in_archive);
        }
    }

    // ---------- rendering ----------

    /// Column widths (modified, created, type, size, tags, git, sync): the live drag, else Settings, with every column nobody has
    /// dragged at the width its content measures (`auto_cols.rs`).
    /// …then the commit columns (message, author, date, hash) at 7..11.
    fn col_widths(&self, cx: &App) -> [f32; 11] {
        self.col_live.unwrap_or_else(|| {
            let c = &cx.global::<crate::settings::Settings>().columns;
            let (a, b, g) = (c.widths, c.extra_widths, c.git_widths);
            self.auto_cols.apply([a[0], a[1], a[2], a[3], b[0], b[1], b[2], g[0], g[1], g[2], g[3]].map(f32::from))
        })
    }

    /// The Name column is about a third of the list by default in the Explorer look; otherwise the other columns leave the spare room.
    fn name_fixed(&self, cx: &App) -> Option<f32> {
        self.name_live.or_else(|| {
            let w = cx.global::<crate::settings::Settings>().columns.name_width;
            if w > 0 {
                Some(f32::from(w))
            } else if cx.global::<crate::settings::Settings>().windows_file_explorer_theme {
                Some((f32::from(self.content_width) * 0.33).clamp(180., 480.))
            } else {
                None
            }
        })
    }

    /// The Name column's width now, after its saved and live widths are considered.
    fn name_now(&self, cx: &App) -> f32 {
        self.name_fixed(cx).unwrap_or_else(|| 180. + self.spare_width(cx))
    }

    /// Which Details columns show: the ones turned on that make sense here (git inside a
    /// repository, sync with cloud files, location in search), then as many as fit the pane
    /// in priority order — the name column keeps at least 180 px.
    fn visible_cols(&self, cx: &App) -> Cols {
        let c = &cx.global::<crate::settings::Settings>().columns;
        const PINNED: u32 = 0x80000;
        let cloud = c.sync && self.location().path().is_some() && {
            match self.cloud_cache.get() {
                Some((g, any)) if g == self.entries_gen => any,
                _ => {
                    let any = self.entries.iter().any(|e| e.is_cloud_placeholder() || e.attrs & PINNED != 0);
                    self.cloud_cache.set(Some((self.entries_gen, any)));
                    any
                }
            }
        };
        let w = self.col_widths(cx);
        // the property columns the user added are placed first: the fixed columns share what is left
        let mut free = f32::from(self.content_width) - self.name_fixed(cx).unwrap_or(180.) - 24. - self.gutter() - self.props_used_width(cx);
        let mut take = |want: bool, width: f32| -> bool {
            if want && width <= free {
                free -= width;
                true
            } else {
                false
            }
        };
        let size = take(c.size, w[3]);
        let modified = take(c.modified, w[0]);
        let kind = take(c.kind, f32::from(self.type_col_w(cx)));
        let git = take(c.git && self.git.is_some(), w[5]);
        // (the tag column only where an item has a tag, as the Git column only inside a repository)
        let tags = take(c.tags && !self.is_search() && self.auto_cols.tags_any, w[4]);
        let sync = take(cloud, w[6]);
        let created = take(c.created && !self.is_search(), w[1]);
        let repo = self.git.is_some() && !self.is_search();
        let message = take(c.git_message && repo, w[7]);
        let date = take(c.git_date && repo, w[9]);
        let author = take(c.git_author && repo, w[8]);
        let hash = take(c.git_hash && repo, w[10]);
        Cols { modified, created, kind, size, tags, git, sync, commit: [message, author, date, hash] }
    }

    /// Width the visible Details columns leave unused (what [`Self::visible_cols`] still had free).
    fn spare_width(&self, cx: &App) -> f32 {
        let vis = self.visible_cols(cx);
        let w = self.col_widths(cx);
        let mut used = 0.;
        for (on, width) in [(vis.modified, w[0]), (vis.created, w[1]), (vis.size, w[3]), (vis.tags, w[4]), (vis.git, w[5]), (vis.sync, w[6])] {
            if on {
                used += width;
            }
        }
        if vis.kind {
            used += f32::from(self.type_col_w(cx));
        }
        used += self.props_used_width(cx);
        for (i, on) in vis.commit.iter().enumerate() {
            if *on {
                used += w[7 + i];
            }
        }
        (f32::from(self.content_width) - self.name_fixed(cx).unwrap_or(180.) - 24. - self.gutter() - used).max(0.)
    }

    fn render_header(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let w = self.col_widths(cx);
        let search = self.is_search();
        // The drag handle on a column's RIGHT edge sizes that column and moves the ones after it, as in Explorer (issue 135: "Dragging
        // will adjust the width of the left column, while moving the ones to the right together. Double-clicking will auto adjust the
        // left column width"). The Name column has one too (`NAME_COL`).
        // (inside the column, with the line at its edge: the part of a handle that reached into the next column lay under that
        // column's header, which takes the press)
        let handle = |ix: usize, cx: &mut Context<Self>| {
            div()
                .id(("col-resize", ix))
                .absolute()
                .right_0()
                .top_0()
                .h_full()
                .w(px(8.))
                .flex()
                .justify_end()
                .cursor_col_resize()
                .child(div().w(px(1.)).h_full().bg(p.divider))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        if ev.click_count >= 2 {
                            // a double click on the edge fits the column to its content (as Explorer does)
                            if ix == NAME_COL {
                                this.fit_name_col(window, cx);
                            } else {
                                this.reset_col_width(ix, cx);
                            }
                            return;
                        }
                        // from the first drag on, the Name column keeps its width and the columns stand from the left: a column that
                        // grows then pushes the ones after it instead of eating into the names
                        let name = this.name_now(cx).round();
                        if this.name_fixed(cx).is_none() {
                            this.name_live = Some(name);
                        }
                        let cur = if ix == NAME_COL { name } else { this.col_widths(cx)[ix] };
                        this.drag_room = (name, this.spare_width(cx));
                        this.col_drag = Some((ix, f32::from(ev.position.x), cur));
                    }),
                )
                .on_drag(ColumnResize, |_, _, _, cx| cx.new(|_| ColumnGhost))
        };
        let vis = self.visible_cols(cx);
        let (show_tags, show_git, show_sync) = (vis.tags, vis.git, vis.sync);
        // columns that don't sort: plain label + resize handle
        let plain = |id: &'static str, label: &'static str, width: f32| {
            h_flex()
                .id(id)
                .h_full()
                .px_2()
                .items_center()
                .w(px(width))
                .flex_shrink_0()
                .overflow_hidden()
                .text_color(p.text_secondary)
                .child(div().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label))
                .relative()
        };
        let col = |id: &'static str, label: &'static str, key: SortKey, w: Option<Pixels>, right: bool, cx: &mut Context<Self>| {
            let active = self.sort.key == key && !self.props.sort_active();
            let arrow = if self.sort.dir == SortDir::Asc { L::ChevronUp } else { L::ChevronDown };
            let mut d = h_flex()
                .id(id)
                .h_full()
                .when(cx.global::<crate::settings::Settings>().windows_file_explorer_theme, |d| d.pl(px(4.)).pr_2().gap_1())
                .when(!cx.global::<crate::settings::Settings>().windows_file_explorer_theme, |d| d.px_2().gap_2())
                .items_center()
                .rounded(theme::RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(p.hover))
                .text_color(if active { p.text } else { p.text_secondary })
                .when(right, |d| d.justify_end())
                .overflow_hidden()
                .child(div().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label))
                .when(active, |d| d.child(Icon::new(arrow).size(px(12.)).flex_shrink_0().text_color(p.text_secondary)))
                .on_click(cx.listener(move |this, _, _, cx| this.set_sort(key, cx)));
            d = match w {
                Some(w) => d.w(w).flex_shrink_0(),
                None => d.flex_1().min_w(px(120.)),
            };
            d
        };
        h_flex()
            .h(px(if cx.global::<crate::settings::Settings>().windows_file_explorer_theme { 32. } else { 30. }))
            .pl(px(10.))
            .pr(px(10. + self.gutter()))
            .overflow_hidden()
            .text_xs()
            .border_b_1()
            .border_color(p.divider)
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ColumnResize>, _, cx| {
                let Some((ix, sx, sw)) = this.col_drag else { return };
                let dx = f32::from(e.event.position.x) - sx;
                // no wider than the room left at the right end: a column that no longer fits is hidden, so widening one past it would
                // make the last one vanish under the pointer
                let (name0, spare0) = this.drag_room;
                if ix == NAME_COL {
                    // the Name column may take the whole width: the columns that no longer fit after it are hidden, as in a narrow
                    // window (Explorer scrolls sideways instead; this list has no sideways scrolling)
                    let most = f32::from(this.content_width) - 24. - this.gutter() - this.props_used_width(cx);
                    this.name_live = Some((sw + dx).clamp(120., most.max(120.)).round());
                } else {
                    let mut w = this.col_widths(cx);
                    // into the room at the right end first, then the Name column gives way, down to 180 px
                    let most = (sw + spare0 + (name0 - 180.).max(0.)).clamp(sw.min(600.), 600.);
                    w[ix] = (sw + dx).clamp(70., most.max(70.)).round();
                    let past = (w[ix] - sw - spare0).max(0.);
                    this.name_live = Some((name0 - past).max(180.).min(name0).round());
                    this.col_live = Some(w);
                    this.col_dragged |= 1 << ix;
                }
                this.schedule_col_save(cx);
                cx.notify();
            }))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<prop_cols::PropResize>, _, cx| this.props_drag_move(e, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.show_header_menu(ev.position, window, cx)))
            .child(
                col("h-name", t!("Ad"), SortKey::Name, self.name_fixed(cx).map(px), false, cx)
                    .when(cx.global::<crate::settings::Settings>().windows_file_explorer_theme, |d| d.pl(px(38.)))
                    .relative()
                    .child(handle(NAME_COL, cx)),
            )
            .children(show_tags.then(|| self.sortx_header("h-tags", t!("Dosya etiketi"), w[4], SortKey::Tag, p, cx).child(handle(4, cx))))
            .children(show_git.then(|| plain("h-git", t!("Git durumu"), w[5]).child(handle(5, cx))))
            .children(show_sync.then(|| self.sortx_header("h-sync", t!("Eşitleme durumu"), w[6], SortKey::Sync, p, cx).child(handle(6, cx))))
            .children(vis.commit[0].then(|| plain("h-gmsg", t!("Son commit mesajı"), w[7]).child(handle(7, cx))))
            .children(vis.commit[1].then(|| plain("h-gauth", t!("Commit yazarı"), w[8]).child(handle(8, cx))))
            .children(vis.commit[2].then(|| plain("h-gdate", t!("Commit tarihi"), w[9]).child(handle(9, cx))))
            .children(vis.commit[3].then(|| plain("h-ghash", t!("Commit hash"), w[10]).child(handle(10, cx))))
            .children(vis.modified.then(|| {
                col("h-mod", t!("Değiştirme tarihi"), SortKey::Modified, Some(px(w[0])), false, cx).relative().child(handle(0, cx))
            }))
            .children(vis.created.then(|| {
                col("h-created", t!("Oluşturma tarihi"), SortKey::Created, Some(px(w[1])), false, cx).relative().child(handle(1, cx))
            }))
            .children(vis.kind.then(|| {
                // (in results from several folders this column is the folder, and it sorts by it)
                let c = col("h-type", if search { t!("Konum") } else { t!("Tür") }, if search { SortKey::Path } else { SortKey::Type }, Some(self.type_col_w(cx)), false, cx).relative();
                if search { c } else { c.child(handle(2, cx)) }
            }))
            .children(vis.size.then(|| {
                col("h-size", t!("Boyut"), SortKey::Size, Some(px(w[3])), !cx.global::<crate::settings::Settings>().windows_file_explorer_theme, cx)
                    .relative()
                    .child(handle(3, cx))
            }))
            .children(self.props_header_cells(p, cx))
            // (the room left over, once the Name column has a width of its own)
            .when(self.name_fixed(cx).is_some(), |d| d.child(div().flex_1()))
    }

    fn store_col_width(c: &mut crate::settings::ColumnPrefs, ix: usize, width: u16) {
        match ix {
            0..=3 => c.widths[ix] = width,
            4..=6 => c.extra_widths[ix - 4] = width,
            _ => c.git_widths[(ix - 7).min(3)] = width,
        }
    }

    /// A double click on the Name column's right edge: as wide as the longest name shown (its icon and padding included), within the
    /// room there is; the other columns keep their widths.
    fn fit_name_col(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.col_drag = None;
        let style = window.text_style();
        let size = style.font_size.to_pixels(window.rem_size());
        // (the longest names by their letters: measuring every one of 100 000 would hold the frame)
        let mut names: Vec<&str> = self.visible.iter().filter_map(|&i| self.entries.get(i as usize)).map(|e| e.name.as_ref()).collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
        let widest = names
            .iter()
            .take(400)
            .map(|n| f32::from(window.text_system().layout_line(n, size, &[style.to_run(n.len())], None).width))
            .fold(0., f32::max);
        let checkbox = if cx.global::<crate::settings::Settings>().show_checkboxes { 24. } else { 0. };
        // icon 16 + gap 8 + the cell's padding 16, and a little air
        let want = (widest + 16. + 8. + 16. + checkbox + 12.).round();
        let most = self.name_now(cx) + self.spare_width(cx);
        let w = want.clamp(120., most.max(120.));
        self.name_live = None;
        crate::settings::update(cx, |s| s.columns.name_width = w as u16);
        cx.notify();
    }

    /// The column at `ix` gets its stock width again, which for most columns means "as wide as its content" (`auto_cols.rs`).
    fn reset_col_width(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.col_drag = None;
        self.col_live = None;
        self.col_dragged = 0;
        self.col_save_gen += 1;
        let d = crate::settings::ColumnPrefs::default();
        let stock = match ix {
            0..=3 => d.widths[ix],
            4..=6 => d.extra_widths[ix - 4],
            _ => d.git_widths[(ix - 7).min(3)],
        };
        crate::settings::update(cx, |s| Self::store_col_width(&mut s.columns, ix, stock));
        cx.notify();
    }

    /// Writes dragged column widths to Settings once the drag pauses (a save per move would
    /// rewrite settings.toml dozens of times a second).
    fn schedule_col_save(&mut self, cx: &mut Context<Self>) {
        self.col_save_gen += 1;
        let generation = self.col_save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
            let _ = this.update(cx, |v, cx| {
                if v.col_save_gen != generation {
                    return;
                }
                if let Some(n) = v.name_live.take() {
                    v.col_drag = None;
                    crate::settings::update(cx, |s| s.columns.name_width = n.round().clamp(120., 4000.) as u16);
                }
                if let Some(w) = v.col_live.take() {
                    v.col_drag = None;
                    // only the columns that were dragged: the live widths of the others are measured ones, and storing them would
                    // end their being automatic
                    let dragged = std::mem::take(&mut v.col_dragged);
                    crate::settings::update(cx, |s| {
                        for ix in (0..w.len()).filter(|ix| dragged & (1 << ix) != 0) {
                            Self::store_col_width(&mut s.columns, ix, auto_cols::kept(ix, w[ix]));
                        }
                    });
                }
            });
        })
        .detach();
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = Palette::for_window(window);
        let search_mode = self.is_search();
        let prefs = cx.global::<crate::settings::Settings>();
        let show_ext = prefs.show_extensions;
        let renamed = self.renamed.clone();
        let checkboxes = prefs.show_checkboxes;
        let hover_select = prefs.select_on_hover;
        // folders inside an archive have no size of their own to measure
        let measurable = !self.location().path().is_some_and(crate::archive_view::is_browsable_path);
        let folder_sizes = prefs.calculate_folder_sizes;
        let on_disk = prefs.folder_size_on_disk;
        let vis = self.visible_cols(cx);
        let show_date = vis.modified;
        let show_created = vis.created;
        let show_type = vis.kind;
        let show_size = vis.size;
        let type_w = self.type_col_w(cx);
        let cw = self.col_widths(cx);
        let name_w = self.name_fixed(cx);
        let grouped = self.grouped();
        let row_h = self.row_h();
        let (col_tags, col_git, col_sync) = (vis.tags, vis.git, vis.sync);
        // columns for Windows properties: None (and nothing else below runs) when the user has added none
        let prop_ctx = self.props_row_ctx(cx);
        let mut shown: Option<(usize, usize)> = None;
        let mut out = Vec::with_capacity(range.len());
        for r in range {
            let vi = if grouped {
                match self.rows.get(r).cloned() {
                    Some(Row::Item(vi)) => vi,
                    Some(Row::Header(label, n)) => {
                        let folded = self.collapsed.contains(label.as_ref());
                        let key = label.to_string();
                        out.push(
                            h_flex()
                                .id(("grp", r))
                                .w_full()
                                .h(row_h)
                                .px_2()
                                .pt_2()
                                .gap_2()
                                .items_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_secondary)
                                .cursor_pointer()
                                .hover(|s| s.text_color(p.text))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&key, cx)))
                                .child(Icon::new(if folded { L::ChevronRight } else { L::ChevronDown }).size(px(12.)))
                                .child(crate::i18n::td(&label))
                                .child(div().text_color(p.text_tertiary).child(format!("{n}")))
                                .child(div().flex_1().h(px(1.)).bg(p.divider))
                                .into_any_element(),
                        );
                        continue;
                    }
                    None => break,
                }
            } else {
                r
            };
            let Some(&ei) = self.visible.get(vi) else { break };
            let prop_cells = match &prop_ctx {
                Some(c) => {
                    shown = Some(shown.map_or((vi, vi), |(a, b)| (a.min(vi), b.max(vi))));
                    self.props_cells(c, ei, &p)
                }
                None => Vec::new(),
            };
            // Folder sizes: every visible one with Settings → "Calculate folder sizes", otherwise
            // only those clicked ("Boyutu hesapla"); measured in the background.
            let dir_path = (show_size && measurable && self.entries[ei as usize].is_dir()).then(|| self.path_of(ei)).flatten();
            let dir_size: Option<String> = dir_path.clone().and_then(|p| {
                if folder_sizes {
                    Some(self.folder_size(p, cx).text(on_disk))
                } else {
                    self.folder_sizes.get(&p).map(|s| s.text(on_disk))
                }
            });
            let calc_link = dir_path.filter(|_| dir_size.is_none());
            let e = &self.entries[ei as usize];
            let selected = self.selected.contains(&ei);
            // (the rows next to it that are selected too: the pill joins them)
            let sel_above = selected && vi > 0 && self.visible.get(vi - 1).is_some_and(|e| self.selected.contains(e));
            let sel_below = selected && self.visible.get(vi + 1).is_some_and(|e| self.selected.contains(e));
            let is_cursor = self.cursor == Some(vi);
            let cat = icons::category(e);
            let dim = e.is_hidden() || e.is_cloud_placeholder();
            let name: SharedString = crate::rtl::display_text(&row_name(e, show_ext, renamed.as_ref())).into();
            let shell_icon = self
                .parent_of(ei)
                .map(|d| d.to_path_buf())
                .and_then(|d| self.images.update(cx, |c, cx| c.icon(&d, e, cx)));
            let overlay = self.overlay_of(ei, px(16.), cx);
            let e = &self.entries[ei as usize];
            let git_state = self.git.as_ref().and_then(|g| rf_win::git::state_for_child(g, self.parent_of(ei)?, &e.name));
            let git_cell = col_git.then(|| {
                let (label, color) = match git_state {
                    Some(st) => (git_label(st), git_style(st).1),
                    None => ("", p.text_secondary),
                };
                div().w(px(cw[5])).flex_shrink_0().px_2().text_color(color).whitespace_nowrap().overflow_hidden().text_ellipsis().child(label)
            });
            let git_badge = (!col_git).then_some(()).and(self.git.as_ref()).and_then(|g| {
                let dir = self.parent_of(ei)?;
                let st = rf_win::git::state_for_child(g, dir, &e.name)?;
                let (letter, color) = git_style(st);
                Some(
                    div()
                        .flex_shrink_0()
                        .px_1()
                        .rounded(px(3.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(letter),
                )
            });
            // commit columns: only for the current folder's own children
            let commit = vis
                .commit
                .iter()
                .any(|&b| b)
                .then_some(self.git_commits.as_ref())
                .flatten()
                .filter(|_| self.parent_of(ei) == self.location().path())
                .and_then(|m| m.get(&*e.name));
            let commit_cells: Vec<AnyElement> = (0..4)
                .filter(|&i| vis.commit[i])
                .map(|i| {
                    let text: String = match (i, commit) {
                        (_, None) => String::new(),
                        (0, Some(c)) => c.message.clone(),
                        (1, Some(c)) => c.author.clone(),
                        (2, Some(c)) => format::datetime(c.time_ms),
                        (_, Some(c)) => c.sha.clone(),
                    };
                    div()
                        .w(px(cw[7 + i]))
                        .flex_shrink_0()
                        .px_2()
                        .text_color(p.text_secondary)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(text)
                        .into_any_element()
                })
                .collect();
            let tag_cell = col_tags.then(|| {
                let store = &cx.global::<crate::tags::Tags>().0;
                let ids: Vec<u32> = self.path_of(ei).map(|path| store.tags_of(&path).to_vec()).unwrap_or_default();
                let mut cell = h_flex().w(px(cw[4])).flex_shrink_0().px_2().gap_1().items_center().overflow_hidden().text_color(p.text_secondary);
                for (i, id) in ids.iter().enumerate() {
                    if let Some(d) = store.def(*id) {
                        cell = cell.child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(crate::tags::color(d.color)));
                        if i == 0 {
                            cell = cell.child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(crate::i18n::td(&d.name)));
                        }
                    }
                }
                cell
            });
            let tag_dots = (!col_tags).then_some(()).and(self.path_of(ei)).and_then(|path| {
                let store = &cx.global::<crate::tags::Tags>().0;
                let ids = store.tags_of(&path);
                (!ids.is_empty()).then(|| {
                    let mut row = h_flex().gap_0p5().flex_shrink_0();
                    for id in ids {
                        if let Some(d) = store.def(*id) {
                            row = row.child(div().size(px(8.)).rounded_full().bg(crate::tags::color(d.color)));
                        }
                    }
                    row
                })
            });
            // OneDrive & co. (Files' sync status): online-only = cloud, always kept here = check
            let sync_cell = col_sync.then(|| {
                const PINNED: u32 = 0x80000;
                let (icon, color, label) = if e.is_cloud_placeholder() {
                    (Some(L::Cloud), p.accent, t!("Yalnızca çevrimiçi"))
                } else if e.attrs & PINNED != 0 {
                    (Some(L::CircleCheck), gpui_kit::rgb(0x2EA043).into(), t!("Her zaman bu cihazda"))
                } else {
                    (Some(L::CircleCheck), p.text_tertiary, t!("Bu cihazda kullanılabilir"))
                };
                h_flex()
                    .w(px(cw[6]))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .items_center()
                    .overflow_hidden()
                    .text_color(p.text_secondary)
                    .children(icon.map(|ic| Icon::new(ic).size(px(14.)).text_color(color)))
                    .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label))
            });
            let cloud_badge = if col_sync { None } else {
                const PINNED: u32 = 0x80000;
                let (icon, color, tip) = if e.is_cloud_placeholder() {
                    (Some(L::Cloud), p.accent, t!("Yalnızca çevrimiçi"))
                } else if e.attrs & PINNED != 0 {
                    (Some(L::CircleCheck), gpui_kit::rgb(0x2EA043).into(), t!("Her zaman bu cihazda"))
                } else {
                    (None, p.text, "")
                };
                icon.map(|ic| {
                    div()
                        .id(("cloud", vi))
                        .flex_shrink_0()
                        .px_1()
                        .child(Icon::new(ic).size(px(14.)).text_color(color))
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
                })
            };
            let rename_input = self.renaming.as_ref().filter(|(i, _)| *i == ei).map(|(_, inp)| inp.clone());
            let name_el: AnyElement = match rename_input {
                // the box grows to the left (border + padding) so the text keeps its place
                Some(inp) => div()
                    .capture_key_down({
                        let input = inp.clone();
                        cx.listener(move |_, ev: &KeyDownEvent, _, cx| crate::rtl::handle_key_down(&input, ev, cx))
                    })
                    .flex_1()
                    // (as tall as the row at the tightest size)
                    .h(row_h.min(px(24.)))
                    .ml(px(-RENAME_INSET))
                    .px(px(RENAME_INSET - 1.))
                    .rounded(px(4.))
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.accent)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // a press anywhere else applies the new name, as in Explorer (issue 180)
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| this.commit_rename_if_open(window, cx)))
                    .child(gpui_kit::component::input::Input::new(&inp).appearance(false).small().px_0())
                    .into_any_element(),
                None => div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(dim, |d| d.opacity(0.6))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _, _| this.name_hit = true))
                    .child(name)
                    .into_any_element(),
            };
            let row = h_flex()
                .id(("row", vi))
                .group("rowgrp")
                .h(row_h)
                .w_full()
                .px(px(4.))
                .rounded(theme::RADIUS_SM)
                .items_center()
                .text_sm()
                .relative()
                .when(selected, |d| d.bg(p.selection).child(theme::selection_pill_joined(&p, sel_above, sel_below)))
                .when(!selected, |d| d.hover(|s| s.bg(p.hover)))
                .when(is_cursor && selected && self.selected.len() > 1, |d| {
                    d.border_1().border_color(p.selection_border)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.on_row_mouse_down(vi, ev, window, cx)),
                )
                .on_mouse_up(MouseButton::Left, cx.listener(move |this, ev: &MouseUpEvent, _, cx| this.on_row_mouse_up(vi, ev, cx)))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| this.on_row_right_down(vi, window, cx)),
                )
                .when_some(self.path_of(ei), |d, path| file_drop_target(d, path, e.is_dir(), &p))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        let Some(&ei) = this.visible.get(vi) else { return };
                        if this.entries[ei as usize].is_dir() {
                            if let Some(p) = this.path_of(ei) {
                                cx.emit(FolderEvent::OpenInNewTab(Location::Path(p)));
                            }
                        }
                    }),
                )
                .child(
                    h_flex()
                        .map(|d| match name_w {
                            Some(w) => d.w(px(w)).flex_shrink_0(),
                            None => d.flex_1().min_w(px(120.)),
                        })
                        .when(cx.global::<crate::settings::Settings>().windows_file_explorer_theme, |d| d.pl(px(22.)).pr_2().gap_1p5())
                        .when(!cx.global::<crate::settings::Settings>().windows_file_explorer_theme, |d| d.px_2().gap_2())
                        .items_center()
                        .overflow_hidden()
                        // Settings → "Show checkboxes when selecting items"
                        .when(checkboxes, |d| {
                            d.child(
                                div()
                                    .id(("chk", vi))
                                    .size(px(16.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .border_1()
                                    .when(selected, |d| d.bg(p.accent).border_color(p.accent))
                                    .when(!selected, |d| d.border_color(p.text_tertiary).invisible().group_hover("rowgrp", |s| s.visible()))
                                    .when(selected, |d| d.child(Icon::new(L::Check).size(px(11.)).text_color(p.accent_text)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            let Some(&ei) = this.visible.get(vi) else { return };
                                            if !this.selected.remove(&ei) {
                                                this.selected.insert(ei);
                                            }
                                            this.cursor = Some(vi);
                                            this.anchor = Some(vi);
                                            cx.notify();
                                        }),
                                    ),
                            )
                        })
                        .child(download_marked(link_marked(
                            match &shell_icon {
                                Some(im) => img(im.clone()).size(px(16.)).flex_shrink_0().into_any_element(),
                                None => Icon::new(icons::icon_for(cat))
                                    .size(px(16.))
                                    .flex_shrink_0()
                                    .text_color(theme::kind_color(cat, p.dark))
                                    .into_any_element(),
                            },
                            px(16.),
                            e.link,
                            overlay,
                        ), self.part_frac(ei)))
                        .child(name_el)
                        // encrypted: an archive member that needs a password, or an EFS file
                        .children((e.attrs & rf_core::attrs::ENCRYPTED != 0).then(|| Icon::new(L::Lock).size(px(12.)).flex_shrink_0().text_color(p.text_secondary)))
                        .children(tag_dots)
                        .children(git_badge)
                        .child(div().flex_1())
                        .children(cloud_badge),
                )
                .children(tag_cell)
                .children(git_cell)
                .children(sync_cell)
                .children(commit_cells)
                .children(show_date.then(|| {
                    div()
                        .w(px(cw[0]))
                        .flex_shrink_0()
                        .px_2()
                        .text_color(p.text_secondary)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(format::datetime(e.modified_ms))
                }))
                .children(show_created.then(|| {
                    div()
                        .w(px(cw[1]))
                        .flex_shrink_0()
                        .px_2()
                        .text_color(p.text_secondary)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(format::datetime(e.created_ms))
                }))
                .when(show_type, |row| row.child(
                    div()
                        .w(type_w)
                        .flex_shrink_0()
                        .px_2()
                        .text_color(p.text_secondary)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(if search_mode {
                            self.parent_of(ei).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default()
                        } else {
                            icons::type_label(e)
                        }),
                ))
                .when(show_size, |row| {
                    let cell = div().w(px(cw[3])).flex_shrink_0().px_2().flex().justify_end().whitespace_nowrap().overflow_hidden();
                    let cell = match calc_link {
                        // "Boyutu hesapla": measures just this folder (doesn't select the row)
                        Some(dir) => cell.child(
                            div()
                                .id(("calc-size", vi))
                                .text_xs()
                                .text_color(p.text_tertiary)
                                .cursor_pointer()
                                .hover(|s| s.text_color(p.accent).underline())
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(t!("Boyutu hesapla"))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.folder_size(dir.clone(), cx);
                                        cx.notify();
                                    }),
                                ),
                        ),
                        None => {
                            let text = if !e.is_dir() { format::size(e.size) } else { dir_size.clone().unwrap_or_default() };
                            cell.text_color(p.text_secondary).child(text)
                        }
                    };
                    row.child(cell)
                })
                .children(prop_cells)
                .when(name_w.is_some(), |row| row.child(div().flex_1()))
                // Settings → "Select files and folders on hover"
                .when(hover_select, |row| row.on_hover(cx.listener(move |this, on: &bool, _, cx| this.hover_row(vi, *on, cx))));
            out.push(row.into_any_element());
        }
        self.props_after_rows(shown, prop_ctx.is_some(), cx);
        out
    }

    fn render_details(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let count = self.visible.len();
        let list_rows_before = std::mem::take(&mut self.list_rows);
        // (every group folded leaves no item but the groups' headers: that is not an empty folder)
        let shown = count.max(self.rows.len());
        let mut side_bar = false;
        // Details and Grid get a scrollbar over the list (Columns has one list per column).
        let mut list_body = false;
        let body: AnyElement = if let Some(err) = &self.error {
            empty_state(p, L::TriangleAlert, crate::i18n::td(err), t!("Klasör açılamadı")).into_any_element()
        } else if shown == 0 && self.loading && self.load_started.elapsed() >= SLOW_LOAD {
            let text = self.search_stage.map_or_else(|| t!("Yükleniyor…").to_string(), |s| s.text().to_string());
            empty_state(p, L::LoaderCircle, text, "").into_any_element()
        } else if shown == 0 && !self.loading {
            // (a search that found nothing is no empty folder)
            let msg = if self.filter.query.is_empty() && !matches!(self.location(), Location::Search { .. }) { t!("Bu klasör boş") } else { t!("Eşleşen öğe yok") };
            empty_state(p, L::FolderOpen, msg.to_string(), "").into_any_element()
        } else if self.view_mode == ViewMode::Columns {
            self.render_columns(p, cx)
        } else if self.column_list(cx) {
            side_bar = true;
            self.render_column_list(list_rows_before, window, cx)
        } else if self.view_mode == ViewMode::Grid {
            let avail = self.content_width - px(12.);
            let cols = ((avail / (self.grid_tile_w() + GRID_GAP)).floor() as usize).max(1);
            self.grid_cols = cols;
            let rows = if self.grid_grouped() { self.grid_units_now().0.len() } else { count.div_ceil(cols) };
            list_body = true;
            uniform_list("grid", rows, cx.processor(|this, range, window, cx| this.render_grid_rows(range, window, cx)))
                .track_scroll(&self.scroll)
                .flex_1()
                .py_2()
                .px(px(6.))
                .into_any_element()
        } else {
            let n_rows = if self.grouped() { self.rows.len() } else { count };
            list_body = true;
            uniform_list("rows", n_rows, cx.processor(|this, range, window, cx| this.render_rows(range, window, cx)))
                .track_scroll(&self.scroll)
                .flex_1()
                .py_1()
                .pl(px(6.))
                .pr(px(6. + self.gutter()))
                .into_any_element()
        };
        let this = cx.entity();
        let width_probe = canvas(
            move |bounds, window, cx| {
                let changed = this.update(cx, |v, cx| {
                    let changed = v.content_width != bounds.size.width;
                    if changed {
                        v.content_width = bounds.size.width;
                        cx.notify();
                    }
                    changed
                });
                // Width-dependent layout (columns, grid) was built with the old width: redraw.
                if changed {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let header = (self.view_mode == ViewMode::Details).then(|| self.render_header(p, cx));
        v_flex()
            .size_full()
            .relative()
            .child(width_probe)
            .children(header)
            .child(
                div()
                    .id("rows-bg")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            // Settings → "Double-click an empty area to go up": when both clicks were made on the empty area of
                            // this listing (a double click that opened this folder does not also leave it)
                            let double = ev.click_count == 2 && completes_double(this.first_click.as_ref(), this.load_serial, None);
                            this.first_click = (ev.click_count == 1).then(|| FirstClick { load: this.load_serial, name: None });
                            if double && cx.global::<crate::settings::Settings>().double_click_up {
                                this.up(cx);
                                return;
                            }
                            if !(ev.modifiers.control || ev.modifiers.shift) && !this.selected.is_empty() {
                                this.selected.clear();
                                cx.notify();
                            }
                            this.begin_marquee(ev);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.selected.clear();
                            cx.notify();
                            this.show_context_menu(true, window, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                        if this.marquee.is_some() {
                            if ev.pressed_button == Some(MouseButton::Left) {
                                this.update_marquee(ev.position, cx);
                            } else {
                                this.marquee = None;
                                cx.notify();
                            }
                            return;
                        }
                        this.on_drag_motion(ev, window, cx)
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            this.drag_origin = None;
                            if this.marquee.take().is_some() {
                                cx.notify();
                            }
                        }),
                    )
                    .relative()
                    .children(self.render_marquee(p))
                    // Ctrl and the wheel change the size of the items, as in Explorer (issue 62: "make sure that the mouse wheel can
                    // adjust the size as it is done in windows explorer"); heard before the list, which would scroll instead
                    .child({
                        let me = cx.entity().downgrade();
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let me = me.clone();
                                window.on_mouse_event(move |ev: &ScrollEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Capture || !ev.modifiers.control || !bounds.contains(&ev.position) {
                                        return;
                                    }
                                    let dy = match ev.delta {
                                        ScrollDelta::Lines(p) => p.y * ZOOM_STEP,
                                        ScrollDelta::Pixels(p) => f32::from(p.y),
                                    };
                                    let _ = me.update(cx, |v, cx| {
                                        v.zoom_acc += dy;
                                        while v.zoom_acc >= ZOOM_STEP {
                                            v.zoom_acc -= ZOOM_STEP;
                                            v.zoom(1, cx);
                                        }
                                        while v.zoom_acc <= -ZOOM_STEP {
                                            v.zoom_acc += ZOOM_STEP;
                                            v.zoom(-1, cx);
                                        }
                                    });
                                    cx.stop_propagation();
                                });
                            },
                        )
                        .absolute()
                        .size_full()
                    })
                    // (the tiles move under a still pointer: the hovered tile is found again by the next move, `grid_hover`)
                    .on_scroll(cx.listener(|this, _: &ScrollEvent, _, cx| {
                        if this.grid_hover.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(|this, on: &bool, _, cx| {
                        if !*on && this.grid_hover.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .on_drag_move::<ExternalPaths>(cx.listener(|this, ev: &DragMoveEvent<ExternalPaths>, _, cx| {
                        // (only while the pointer is over the list: every listener hears every move, and the tree's or a tab's
                        // own effect must not be overwritten by the list's)
                        if ev.bounds.contains(&ev.event.position)
                            && let Some(dir) = this.current_dir()
                        {
                            show_drop_effect(ev.drag(cx).paths(), &dir);
                        }
                    }))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        if let Some(dir) = this.current_dir() {
                            crate::workspace::drop_files(paths, dir, cx);
                        }
                    }))
                    .child(body)
                    // last child: painted over the list, so its clicks and drags come first. Shown
                    // while scrolling and fading out after, like File Explorer's on Windows 11,
                    // so it doesn't sit over the right-aligned Size column; always, when Windows is
                    // told to keep scrollbars shown (issue 86)
                    .when(list_body && cx.global::<crate::settings::Settings>().show_scrollbar, |d| {
                        d.child(Scrollbar::vertical(&self.scroll).id("list-scrollbar").mode(scrollbar_mode()))
                    })
                    .when(side_bar && cx.global::<crate::settings::Settings>().show_scrollbar, |d| {
                        let base = self.scroll.0.borrow().base_handle.clone();
                        d.child(Scrollbar::horizontal(&base).id("list-hscrollbar").mode(scrollbar_mode()))
                    }),
            )
            .into_any_element()
    }

    fn render_home(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let places = self.places.read(cx);
        let pinned = places.pinned.clone();
        let drives = places.drives.clone();
        let windows_file_explorer_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let devices = places.devices.clone();
        let network = places.network.clone();
        let (hidden, cleared) = {
            let s = cx.global::<crate::settings::Settings>();
            (s.home_hidden.clone(), s.recent_cleared_ms)
        };
        let shown = |k: &str| !hidden.iter().any(|h| h == k);
        let recent: Vec<places::Recent> = places.recent.iter().filter(|r| r.used_ms > cleared).cloned().collect();
        // every tag with how many items carry it
        let tags: Vec<(u32, String, u32, usize)> = {
            let store = &cx.global::<crate::tags::Tags>().0;
            store.defs.iter().map(|d| (d.id, d.name.clone(), d.color, store.paths_with(d.id).len())).collect()
        };
        let card = |id: ElementId| {
            v_flex()
                .id(id)
                .p_3()
                .gap_2()
                .min_w_0()
                .rounded(theme::RADIUS)
                .bg(p.tile_bg)
                .border_1()
                .border_color(p.border)
                .cursor_pointer()
                .hover(|s| s.bg(p.tile_hover))
        };
        let section = |title: &'static str| div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(title);

        // equal-width columns: a card alone on the last row is as wide as the others
        let avail = f32::from(self.content_width) - 48.; // p_6 on both sides
        let mut pinned_grid = div().grid().grid_cols(tile_cols(avail, 140., 220., 12.)).gap_3();
        for (i, f) in pinned.into_iter().enumerate() {
            let path = f.path.clone();
            let shell_icon = if windows_file_explorer_theme {
                self.images.update(cx, |images, cx| {
                    images.shell_icon(&path.to_string_lossy(), 28, cx)
                })
            } else {
                None
            };
            let icon = match shell_icon {
                Some(image) => img(image).size(px(28.)).into_any_element(),
                None => {
                    let icon = Icon::new(f.icon).size(px(28.));
                    if windows_file_explorer_theme {
                        icon.into_any_element()
                    } else {
                        icon.text_color(p.accent).into_any_element()
                    }
                }
            };
            pinned_grid = pinned_grid.child(
                card(("pin", i).into())
                    .child(icon)
                    .child(div().text_sm().child(crate::i18n::td(&f.title)))
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(Location::Path(path.clone()), cx))),
            );
        }

        let mut drive_grid = div().grid().grid_cols(tile_cols(avail, 240., 340., 12.)).gap_3();
        for (i, d) in drives.into_iter().enumerate() {
            let root = d.root.clone();
            let usage_root = d.root.clone();
            let shell_icon = if windows_file_explorer_theme {
                self.images.update(cx, |images, cx| {
                    images.shell_icon(&root.to_string_lossy(), 28, cx)
                })
            } else {
                None
            };
            let icon = match shell_icon {
                Some(image) => img(image).size(px(28.)).into_any_element(),
                None => {
                    let icon = Icon::new(places::drive_icon(&d)).size(px(28.));
                    if windows_file_explorer_theme {
                        icon.into_any_element()
                    } else {
                        icon.text_color(p.text_secondary).into_any_element()
                    }
                }
            };
            let frac = d.used_fraction();
            let bar_color = if frac > 0.9 { p.danger } else { p.accent };
            let menu_root = root.clone();
            drive_grid = drive_grid.child(
                card(("drive", i).into())
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(icon)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(div().text_sm().child(crate::places::drive_name(&d)))
                                    .child(
                                        div()
                                            .h(px(6.))
                                            .w_full()
                                            .rounded_full()
                                            .bg(p.pressed)
                                            .child(div().h_full().rounded_full().bg(bar_color).w(relative(frac))),
                                    )
                                    .child(div().text_xs().text_color(p.text_secondary).child(if d.total > 0 {
                                        format!("{} {} / {}", format::size(d.free), t!("boş"), format::size(d.total))
                                    } else {
                                        String::new()
                                    })),
                            )
                            // where the space goes, as its own tab (the card itself opens the drive)
                            .child(
                                div()
                                    .id(("drive-usage", i))
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme::RADIUS_SM)
                                    .hover(|s| s.bg(p.hover))
                                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Disk kullanımını göster")).build(w, cx))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.emit(FolderEvent::OpenInNewTab(Location::DiskUsage(usage_root.clone())))
                                    }))
                                    .child(Icon::new(L::ChartPie).size(px(16.)).text_color(p.text_secondary)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(Location::Path(root.clone()), cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(FolderEvent::SidebarMenu(menu_root.clone()));
                        }),
                    ),
            );
        }

        // phones and cameras (not drives): none connected = no group at all
        let has_devices = !devices.is_empty();
        let mut device_grid = div().grid().grid_cols(tile_cols(avail, 240., 340., 12.)).gap_3();
        for (i, d) in devices.into_iter().enumerate() {
            let loc = Location::Shell { parsing: d.parsing.clone(), name: d.name.clone() };
            device_grid = device_grid.child(
                card(("device", i).into())
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(Icon::new(L::Usb).size(px(28.)).text_color(p.text_secondary))
                            .child(div().flex_1().min_w_0().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(d.name)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(loc.clone(), cx))),
            );
        }

        let mut tag_grid = div().grid().grid_cols(tile_cols(avail, 140., 220., 12.)).gap_3();
        let has_tags = !tags.is_empty() && shown("tags");
        for (id, name, color, count) in tags {
            tag_grid = tag_grid.child(
                card(("home-tag", id as usize).into())
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().size(px(10.)).flex_shrink_0().rounded_full().bg(crate::tags::color(color)))
                            .child(div().flex_1().min_w_0().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(crate::i18n::td(&name))),
                    )
                    .child(div().text_xs().text_color(p.text_secondary).child(crate::i18n::items(count)))
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(Location::Tag(id), cx))),
            );
        }

        let mut network_grid = div().grid().grid_cols(tile_cols(avail, 240., 340., 12.)).gap_3();
        let has_network = !network.is_empty() && shown("network");
        for (i, (name, target)) in network.into_iter().enumerate() {
            let shown_target = target.to_string_lossy().into_owned();
            network_grid = network_grid.child(
                card(("home-net", i).into())
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(Icon::new(L::Network).size(px(28.)).text_color(p.text_secondary))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(crate::rtl::display_text(&name)))
                                    .child(div().text_xs().text_color(p.text_secondary).overflow_hidden().text_ellipsis().whitespace_nowrap().child(shown_target)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(Location::Path(target.clone()), cx))),
            );
        }

        let has_recent = !recent.is_empty() && shown("recent");
        let has_devices = has_devices && shown("devices");
        let mut recent_list = v_flex().gap_0p5().max_w(px(900.));
        for (i, r) in recent.into_iter().enumerate() {
            let e = rf_core::Entry::new(&r.name, if r.is_dir { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File }, 0, 0, 0, 0);
            let cat = icons::category(&e);
            let parent = r.target.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let target = r.target.clone();
            let is_dir = r.is_dir;
            // folders under their Explorer name (Downloads → İndirilenler)
            let shown: String = if is_dir { rf_win::localized::label_if_known(&target).map_or_else(|| r.name.clone(), |l| l.to_string()) } else { r.name.clone() };
            recent_list = recent_list.child(
                h_flex()
                    .id(("recent", i))
                    .h(theme::ROW_H)
                    .px_2()
                    .gap_3()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .text_sm()
                    .hover(|s| s.bg(p.hover))
                    .cursor_pointer()
                    .child(Icon::new(icons::icon_for(cat)).size(px(16.)).text_color(theme::kind_color(cat, p.dark)))
                    .child(div().w(px(260.)).flex_shrink_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(shown))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(p.text_tertiary)
                            .child(parent),
                    )
                    .child(div().w(px(130.)).flex_shrink_0().text_color(p.text_secondary).child(format::datetime(r.used_ms)))
                    // dragged out like a row of the list (the owner: "son kullanılanlarda ... drag drop ta olmuyor")
                    .on_mouse_down(MouseButton::Left, {
                        let target = target.clone();
                        cx.listener(move |this, ev: &MouseDownEvent, _, _| this.home_press = Some((target.clone(), ev.position)))
                    })
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, _| {
                        let Some((path, at)) = this.home_press.as_ref() else { return };
                        if ev.pressed_button != Some(MouseButton::Left) {
                            this.home_press = None;
                            return;
                        }
                        let (dx, dy) = (f32::from(ev.position.x - at.x), f32::from(ev.position.y - at.y));
                        if dx * dx + dy * dy > 36.0 {
                            let path = path.clone();
                            this.home_press = None;
                            crate::ops::drag_paths_out(vec![path]);
                        }
                    }))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        this.home_press = None;
                        if is_dir {
                            this.navigate(Location::Path(target.clone()), cx);
                        } else if this.picking {
                            // the Open dialog (the owner: "aç penceresinde ana sayfada son kullanılanlarda dosya seçilemiyor. direkt
                            // açıyor"): a click shows the file selected in its folder, as a click in the list selects it; a double
                            // click picks it, as in the list
                            if ev.click_count() >= 2 {
                                cx.emit(FolderEvent::Picked(vec![target.clone()]));
                            } else if let (Some(dir), Some(name)) = (target.parent(), target.file_name()) {
                                this.set_reselect(name.to_string_lossy().into_owned());
                                this.navigate(Location::Path(dir.to_path_buf()), cx);
                            }
                        } else {
                            open_in_background(target.clone(), cx);
                        }
                    })),
            );
        }

        // the Home page measures its own width (the list's probe isn't drawn here)
        let this = cx.entity();
        let width_probe = canvas(
            move |bounds, window, cx| {
                let changed = this.update(cx, |v, cx| {
                    let changed = v.content_width != bounds.size.width;
                    if changed {
                        v.content_width = bounds.size.width;
                        cx.notify();
                    }
                    changed
                });
                if changed {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id("home")
            .size_full()
            .relative()
            .overflow_y_scroll()
            .child(width_probe)
            .child(
                v_flex()
                    .p_6()
                    .gap_4()
                    .when(shown("quick"), |d| d.child(section(t!("Hızlı Erişim"))).child(pinned_grid).child(div().h_2()))
                    .when(shown("drives"), |d| d.child(section(t!("Sürücüler"))).child(drive_grid))
                    .when(has_devices, |d| d.child(div().h_2()).child(section(t!("Cihazlar"))).child(device_grid))
                    .when(has_network, |d| d.child(div().h_2()).child(section(t!("Ağ konumları"))).child(network_grid))
                    .when(has_tags, |d| d.child(div().h_2()).child(section(t!("Etiketler"))).child(tag_grid))
                    .when(has_recent, |d| {
                        d.child(div().h_2())
                            .child(
                                h_flex().max_w(px(900.)).justify_between().items_center().child(section(t!("Son kullanılanlar"))).child(
                                    div()
                                        .id("recent-clear")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(theme::RADIUS_SM)
                                        .text_xs()
                                        .text_color(p.text_secondary)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(p.hover).text_color(p.text))
                                        .tooltip(|w, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(t!("Listeyi boşaltır; Windows'un kendi son kullanılanlar listesine dokunulmaz")).build(w, cx)
                                        })
                                        .child(t!("Temizle"))
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                                            crate::settings::update(cx, |s| s.recent_cleared_ms = now);
                                            cx.notify();
                                        })),
                                ),
                            )
                            .child(recent_list)
                    }),
            )
            .into_any_element()
    }
}

/// Columns for a row of equal cards: as many as fit at `min` width, but enough that none is
/// wider than `max` (all columns share the width evenly).
fn tile_cols(avail: f32, min: f32, max: f32, gap: f32) -> u16 {
    let fit = ((avail + gap) / (min + gap)).floor().max(1.);
    let need = ((avail + gap) / (max + gap)).ceil().max(1.);
    need.min(fit) as u16
}

#[cfg(test)]
mod tile_tests {
    #[test]
    fn a_cut_name_keeps_its_extension() {
        use super::split_for_ellipsis as split;
        assert_eq!(split("model v1.2 final.safetensors", false), ("model v1.2 final", ".safetensors"));
        assert_eq!(split("README", false), ("README", ""));
        assert_eq!(split(".gitignore", false), (".gitignore", ""));
        assert_eq!(split("backup.2024", true), ("backup.2024", ""), "a folder");
        assert_eq!(split("Notes. And more words here", false), ("Notes. And more words here", ""), "a dot in a sentence");
    }

    use super::{grid_hover_step, tile_cols};

    #[test]
    fn a_sliced_tile_stays_lit_whichever_order_its_slices_are_told_in() {
        // the pointer moves from the tile's first slice to its second: told "on" before "off", or "off" before "on"
        let one = grid_hover_step(None, 3, true);
        assert_eq!(grid_hover_step(grid_hover_step(one, 3, true), 3, false), Some((3, 1)));
        assert_eq!(grid_hover_step(grid_hover_step(one, 3, false), 3, true), Some((3, 1)));
        // onto another tile: the old one's late "off" does not put the new one out
        assert_eq!(grid_hover_step(grid_hover_step(one, 4, true), 3, false), Some((4, 1)));
        // off its last slice: nothing lit
        assert_eq!(grid_hover_step(one, 3, false), None);
    }

    #[test]
    fn cards_stay_between_min_and_max_and_equal() {
        for avail in [300f32, 600., 893., 1000., 1400., 2000.] {
            for (min, max) in [(140f32, 220f32), (240., 340.)] {
                let n = tile_cols(avail, min, max, 12.) as f32;
                let w = (avail - 12. * (n - 1.)) / n;
                assert!(w <= max + 0.01 || n == 1., "{avail}: {n} cols of {w}");
                assert!(w >= min - 0.01 || n == 1., "{avail}: {n} cols of {w}");
            }
        }
        assert_eq!(tile_cols(100., 140., 220., 12.), 1); // narrow: one column
    }

    /// Grouping by year / month puts the newest first; a key the entry can't give (tag, folder)
    /// comes from the caller, and without it everything is one group.
    #[test]
    fn calendar_and_extra_groups() {
        use super::{GroupBy, Row, compute_view};
        use chrono::TimeZone;
        use rf_core::{Entry, EntryKind, entry::Filter, sort::SortSpec};
        let at = |y: i32, m: u32, d: u32| chrono::Local.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap().timestamp_millis();
        let entries = vec![
            Entry::new("eski.txt", EntryKind::File, 1, at(2024, 3, 5), 0, 0),
            Entry::new("yeni.txt", EntryKind::File, 1, at(2026, 11, 11), 0, 0),
            Entry::new("ekim.txt", EntryKind::File, 1, at(2026, 10, 2), 0, 0),
        ];
        let headers = |g: GroupBy, extra: super::Extra| -> Vec<(String, usize)> {
            compute_view(&entries, &Filter::default(), SortSpec::default(), g, extra)
                .rows
                .iter()
                .filter_map(|r| match r {
                    Row::Header(l, n) => Some((l.to_string(), *n)),
                    Row::Item(_) => None,
                })
                .collect()
        };
        assert_eq!(headers(GroupBy::Year, None), vec![("2026".to_string(), 2), ("2024".to_string(), 1)]);
        let months = headers(GroupBy::Month, None);
        assert_eq!(months.len(), 3);
        assert!(months[0].0.ends_with("2026") && months[1].0.ends_with("2026") && months[2].0.ends_with("2024"), "{months:?}");
        assert_ne!(months[0].0, months[1].0);
        assert_eq!(headers(GroupBy::Day, None).len(), 3);
        // the first entry has a tag, the others none
        let tag = |i: u32| if i == 0 { (0, "Kırmızı".to_string()) } else { (u32::MAX, "Etiketsiz".to_string()) };
        assert_eq!(headers(GroupBy::Tag, Some(&tag)), vec![("Kırmızı".to_string(), 1), ("Etiketsiz".to_string(), 2)]);
        assert_eq!(headers(GroupBy::Tag, None).len(), 1);
        // every group has an index of its own, kept in folder layouts
        for (i, (g, _)) in GroupBy::ALL.iter().enumerate() {
            assert_eq!((g.index() as usize, GroupBy::from_index(i as u8)), (i, *g));
        }
        assert_eq!(GroupBy::from_index(200), GroupBy::None);
    }

    /// Filtering the cached sort order gives exactly what sorting the filtered entries gives.
    #[test]
    fn filtering_the_sorted_order_matches_a_full_sort() {
        use super::{GroupBy, compute_view, view_from_sorted};
        use rf_core::{Entry, EntryKind, entry::Filter, sort::{SortKey, SortSpec}};
        let names = ["Rapor 10.pdf", "rapor 2.pdf", "Çizim.png", "notlar.txt", ".gizli", "Proje", "ışık.svg", "RAPOR eski.docx", "a", "b10", "b9"];
        let entries: Vec<Entry> = names
            .iter()
            .enumerate()
            .map(|(i, n)| Entry::new(n, if *n == "Proje" { EntryKind::Dir } else { EntryKind::File }, (i as u64 * 37) % 11, i as i64, 0, 0))
            .collect();
        for key in [SortKey::Name, SortKey::Size, SortKey::Modified] {
            let sort = SortSpec { key, ..Default::default() };
            let base = compute_view(&entries, &Filter::default(), sort, GroupBy::None, None);
            for q in ["", "rapor", "R", "ı", "zzz"] {
                for dot_hidden in [false, true] {
                    let f = Filter { query: q.into(), dot_hidden, ..Default::default() };
                    let full = compute_view(&entries, &f, sort, GroupBy::None, None);
                    let fast = view_from_sorted(&entries, base.sorted.clone(), &f, sort, GroupBy::None, None);
                    assert_eq!(full.visible, fast.visible, "{key:?} {q:?}");
                }
            }
        }
    }
}

/// Files' "Git status" column text.
fn git_label(s: rf_win::git::FileState) -> &'static str {
    use rf_win::git::FileState::*;
    match s {
        Modified => t!("Değiştirildi"),
        Added => t!("Eklendi"),
        Deleted => t!("Silindi"),
        Renamed => t!("Yeniden adlandırıldı"),
        Untracked => t!("İzlenmiyor"),
        Ignored => t!("Yok sayıldı"),
        Conflicted => t!("Çakışma"),
    }
}

fn git_style(s: rf_win::git::FileState) -> (&'static str, Hsla) {
    use rf_win::git::FileState::*;
    let c = |x: u32| -> Hsla { rgb(x).into() };
    match s {
        Modified => ("M", c(0xFF9F0A)),
        Added => ("A", c(0x30D158)),
        Deleted => ("D", c(0xFF453A)),
        Renamed => ("R", c(0x0A84FF)),
        Untracked => ("U", c(0x8E8E93)),
        Ignored => ("I", c(0x636366)),
        Conflicted => ("!", c(0xFF453A)),
    }
}

/// File name as displayed (extension hidden when the setting says so).
fn shown_name(e: &Entry, show_ext: bool) -> String {
    if show_ext || e.is_dir() || e.ext.is_empty() {
        e.shown().to_string()
    } else {
        e.name[..e.stem_len()].to_string()
    }
}

/// A folder's watcher could not be started: said in the log, on the thread that tried (never the interface's: this asks the disk).
/// A place that is no folder has nothing to watch and nothing to warn about, it is shown with its error. One way to get there is
/// an argument another program started us with that is no path at all (in a user's log: "Exportación rápida: C:\Users\User\"),
/// which is shown here once Explorer has handed it back (see `run`).
fn watch_failed(path: &std::path::Path, e: &impl std::fmt::Display) {
    if path.is_dir() {
        log::warn!("watch {path:?}: {e}");
    } else {
        log::debug!("watch {path:?}: not a folder ({e})");
    }
}

/// A name as a row shows it, for a name that is not an entry's yet (the new name of a rename under way): the whole name, or
/// without its extension when extensions are hidden and it is a file with one.
fn shown_new_name(name: &str, is_dir: bool, show_ext: bool) -> String {
    if show_ext || is_dir {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name[..dot].to_string(),
        _ => name.to_string(),
    }
}

/// The name a row shows: the entry's own, or the name it is getting while its rename is under way (`FolderView::renamed`).
fn row_name(e: &Entry, show_ext: bool, renamed: Option<&(Arc<str>, Arc<str>)>) -> String {
    match renamed {
        Some((old, new)) if *e.name == **old => shown_new_name(new, e.is_dir(), show_ext),
        _ => shown_name(e, show_ext),
    }
}

const COL_W: Pixels = px(250.);

/// A read-only column (ancestor or next folder) in the columns view.
fn static_column(
    id: ElementId,
    dir: PathBuf,
    list: Option<Arc<Vec<Entry>>>,
    highlight: Option<String>,
    scroll: Option<UniformListScrollHandle>,
    p: &Palette,
    view: Entity<FolderView>,
) -> impl IntoElement {
    let body: AnyElement = match list {
        None => div().p_3().text_xs().text_color(p.text_tertiary).child(t!("Yükleniyor…")).into_any_element(),
        Some(list) if list.is_empty() => div().p_3().text_xs().text_color(p.text_tertiary).child(t!("Boş")).into_any_element(),
        Some(list) => {
            let n = list.len();
            let pal = *p;
            uniform_list(id.clone(), n, move |range: std::ops::Range<usize>, _, _| {
                range
                    .map(|i| {
                        let e = &list[i];
                        let on = highlight.as_deref() == Some(&*e.name);
                        let cat = icons::category(e);
                        let target = dir.join(&*e.name);
                        let drop_path = target.clone();
                        let is_dir = e.is_dir();
                        let view = view.clone();
                        let row = h_flex()
                            .id(("scol", i))
                            .w_full()
                            .h(px(28.))
                            .px_2()
                            .gap_2()
                            .items_center()
                            .rounded(theme::RADIUS_SM)
                            .text_sm()
                            .when(on, |d| d.bg(pal.hover).font_weight(FontWeight::MEDIUM))
                            .when(!on, |d| d.hover(|s| s.bg(pal.hover)).text_color(pal.text_secondary))
                            .child(Icon::new(icons::icon_for(cat)).size(px(16.)).text_color(theme::kind_color(cat, pal.dark)))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(crate::rtl::display_text(&e.name)))
                            .when(is_dir, |d| d.child(Icon::new(L::ChevronRight).size(px(12.)).text_color(pal.text_tertiary)))
                            // the press is the row's: the list's background under the columns took it as a click on nothing and
                            // cleared the selection, which took this column away before the button came up, and the click was lost
                            // (issue 57: "If I click on one of the folders in the second column, it disappears")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down(MouseButton::Right, {
                                let (view, dir, name) = (view.clone(), dir.clone(), e.name.to_string());
                                move |_, window, cx| {
                                    cx.stop_propagation();
                                    view.update(cx, |v, cx| v.menu_in_column(dir.clone(), name.clone(), window, cx));
                                }
                            })
                            .on_click({
                                let (dir, name) = (dir.clone(), e.name.to_string());
                                move |ev: &ClickEvent, _, cx| {
                                    if is_dir {
                                        view.update(cx, |v, cx| v.navigate(Location::Path(target.clone()), cx));
                                    } else if ev.click_count() >= 2 {
                                        view.update(cx, |v, _| v.static_pick += 1);
                                        open_in_background(target.clone(), cx);
                                    } else {
                                        // one click selects the file, as in the list's own column (issue 118: it opened it)
                                        view.update(cx, |v, cx| v.pick_in_column(dir.clone(), name.clone(), cx));
                                    }
                                }
                            });
                        file_drop_target(row, drop_path, is_dir, &pal).into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .when_some(scroll, |l, h| l.track_scroll(&h))
            .size_full()
            .p_1()
            .into_any_element()
        }
    };
    div()
        .w(COL_W)
        .flex_shrink_0()
        .h_full()
        .border_r_1()
        .border_color(p.divider)
        // a press on the column's empty space is not the list's: the list's background took it as a click on nothing, cleared the
        // selection, and the column of the selected folder went away under the pointer (issue 140)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(body)
}

const GRID_GAP: Pixels = px(4.);
/// The List filled column by column: room above its rows and left of its first column.
const LIST_PAD_TOP: f32 = 8.;
const LIST_PAD_LEFT: f32 = 6.;
/// The List filled column by column: a column's narrowest and widest, what a cell adds to its name (the icon, the gaps and the
/// padding), and the most items whose names are measured for the columns' widths.
const LIST_COL_MIN: f32 = 140.;
const LIST_COL_MAX: f32 = 460.;
const LIST_CELL_CHROME: f32 = 8. + 20. + 8. + 8. + 12.;
const LIST_MEASURE_MOST: usize = 4000;
/// Grid sizes (Ctrl+= / Ctrl+-): tile width, row height, thumbnail pixels.
const GRID_SIZES: [(f32, f32, u32); 5] = [(76., 94., 64), (92., 112., 80), (124., 150., 128), (172., 204., 192), (236., 276., 272)];
const DEFAULT_GRID_SIZE: u8 = 2;
/// Details row heights for the "Size" slider (30 = the classic one; 22 = as tight as File Explorer's compact rows, asked for on Threads).
const ROW_SIZES: [f32; 5] = [22., 30., 36., 44., 52.];
const DEFAULT_ROW_SIZE: u8 = 1;

const SLOW_LOAD: std::time::Duration = std::time::Duration::from_millis(400);

fn empty_state(p: &Palette, icon: L, title: String, sub: &'static str) -> impl IntoElement {
    v_flex()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(Icon::new(icon).size(px(40.)).text_color(p.text_tertiary))
        .child(div().text_color(p.text_secondary).child(title))
        .when(!sub.is_empty(), |d| d.child(div().text_xs().text_color(p.text_tertiary).child(sub)))
}

/// Inline rename: how far the edit box reaches left of the name (its border plus padding), so
/// the text inside starts exactly where the name was drawn.
const RENAME_INSET: f32 = 5.;

/// The empty strip between the rows of the details list and the list's right edge (the preview pane, the other pane, the window).
///
/// WHY (issue 25, with a picture of a selection box squeezed into the last pixels of the Size column): a drag that starts on a row
/// moves the row, so a rubber band can start only where there is no row, and the rows reached the edge: 6 px of the list's own
/// padding were all the room there was. Explorer has such a strip by accident (its columns do not fill the width); here the name
/// column takes what is free, so the strip is made on purpose. The rows, their hover and selection fill and the column headings all
/// end before it; the scrollbar, when shown, lies over its outer part.
///
/// HOW WIDE (the user, of a fixed 24 px: "bu 24px olayı büyük monitörlerde nasıl olacak peki?"): 3 % of the list's width, never
/// under 24 px and never over 56. A fixed strip is the same few millimetres on every screen, since these are the pixels Windows'
/// display scaling multiplies, but beside a list 1600 px wide it is a sliver, and that is where the name column has width to
/// spare. So a narrow list (a small window, two panes, the preview pane open) keeps the 24 px it can afford and a wide one gets
/// up to 56. Pure.
pub(crate) fn details_gutter(list_width: f32) -> f32 {
    (list_width * 0.03).clamp(24., 56.)
}

#[cfg(test)]
mod gutter_tests {
    use super::details_gutter;

    #[test]
    fn the_strip_grows_with_the_list_between_its_two_ends() {
        assert_eq!(details_gutter(0.), 24.);
        assert_eq!(details_gutter(740.), 24., "a 1300 px window with the sidebar and the preview pane");
        assert_eq!(details_gutter(800.), 24.);
        assert_eq!(details_gutter(1200.), 36.);
        assert_eq!(details_gutter(1600.), 48., "the list of the picture in issue 25 (a 2560 px window)");
        assert_eq!(details_gutter(1900.), 56.);
        assert_eq!(details_gutter(4000.), 56.);
    }

    #[test]
    fn it_never_shrinks_while_the_list_widens() {
        let mut last = 0.;
        for w in (0..4000).step_by(10) {
            let g = details_gutter(w as f32);
            assert!(g >= last, "{w}");
            last = g;
        }
    }
}

/// Whether `path` names a `.lnk` shortcut (by its name only).
fn is_lnk(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
}

pub(crate) fn open_in_background(path: PathBuf, cx: &mut App) {
    open_in_background_sorted(path, SortKey::Name, true, cx)
}

/// [`open_in_background`] for a file of a list sorted this way: an app that walks the file's folder (Photos) walks it in this order.
pub(crate) fn open_in_background_sorted(path: PathBuf, key: SortKey, ascending: bool, cx: &mut App) {
    // (the name test first: it costs nothing, and `exists` on a place that has to wake up holds the UI thread)
    if !rf_win::ftp::is_ftp(&path) && crate::nav_touch::may_hold_archive(&path) && !path.exists() && crate::archive_view::split(&path).is_some() {
        // an item inside an archive: extract just it (Status Center), open the real file
        crate::archive_view::open_items(cx, vec![path]);
        return;
    }
    std::thread::spawn(move || {
        // FTP: download to a temp copy, then open that
        let path = if rf_win::ftp::is_ftp(&path) {
            let name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
            let dir = std::env::temp_dir().join("rustyfiles-ftp");
            let _ = std::fs::create_dir_all(&dir);
            let local = dir.join(name);
            if let Err(e) = rf_win::ftp::download(&path.to_string_lossy(), &local) {
                log::warn!("ftp download {path:?}: {e}");
                return;
            }
            local
        } else {
            path
        };
        // (with the user's own program for the file's type when there is one: Settings > File types)
        if let Err(e) = rf_win::openrules::open_in_order(&path, key, ascending) {
            log::warn!("open {path:?}: {e}");
        }
    });
}

impl Render for FolderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // an unfinished download's progress on its icon moves on: drawn again in a moment while one is shown
        if self.parts_shown.replace(false) && !self.part_tick {
            self.part_tick = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                let _ = this.update(cx, |v, cx| {
                    v.part_tick = false;
                    cx.notify();
                });
            })
            .detach();
        }
        if std::mem::take(&mut self.pending_rename) {
            // Create the editor now, but focus it on the next frame: a focus target that is not
            // part of the frame being drawn gets dropped by GPUI.
            self.begin_rename(window, cx);
            cx.on_next_frame(window, |this, window, cx| {
                if let Some((_, inp)) = this.renaming.clone() {
                    inp.update(cx, |s, cx| s.focus(window, cx));
                }
            });
        }
        let p = Palette::for_window(window);
        if self.view_mode == ViewMode::Details {
            self.refresh_auto_cols(window, cx);
        }
        let content = match self.location() {
            Location::Home => self.render_home(&p, cx),
            Location::Settings => crate::settings_view::render(&p, window, cx, self.lens_owner, &self.settings_scroll),
            Location::DiskUsage(root) => {
                let root = root.clone();
                let dv = match &self.disk_usage {
                    Some(dv) if dv.read(cx).root() == root => dv.clone(),
                    _ => {
                        let dv = cx.new(|cx| crate::disk_usage::DiskUsageView::new(root, cx));
                        cx.subscribe(&dv, |this, _, ev: &crate::disk_usage::DiskEvent, cx| match ev {
                            crate::disk_usage::DiskEvent::ShowInFolder(path) => {
                                if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                                    this.set_reselect(name.to_string_lossy().into_owned());
                                    this.navigate(Location::Path(parent.to_path_buf()), cx);
                                }
                            }
                            // (this tab stays on the analysis: the workspace opens the folder in a tab of its own)
                            crate::disk_usage::DiskEvent::OpenInNewTab(path) => cx.emit(FolderEvent::OpenInNewTab(Location::Path(path.clone()))),
                        })
                        .detach();
                        self.disk_usage = Some(dv.clone());
                        dv
                    }
                };
                div().size_full().child(dv).into_any_element()
            }
            Location::Downloads => {
                let dv = match &self.downloads {
                    Some(dv) => dv.clone(),
                    None => {
                        let dv = cx.new(|cx| crate::downloads_view::DownloadsView::new(false, cx));
                        self.downloads = Some(dv.clone());
                        dv
                    }
                };
                div().size_full().child(dv).into_any_element()
            }
            Location::RecycleBin => {
                let rv = match &self.recycle {
                    Some(rv) => rv.clone(),
                    None => {
                        let images = self.images.clone();
                        let rv = cx.new(|cx| crate::recycle_view::RecycleView::new(images, cx));
                        self.recycle = Some(rv.clone());
                        rv
                    }
                };
                div().size_full().child(rv).into_any_element()
            }
            _ => self.render_details(&p, window, cx),
        };
        div()
            .id("folder-view")
            .key_context(FOLDER_CTX)
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|this, _: &BackspaceBack, window, cx| {
                if this.focus.is_focused(window) {
                    this.back(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_cursor(this.row_step(), false, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_cursor(-this.row_step(), false, cx)))
            .on_action(cx.listener(|this, _: &ExtendNext, _, cx| this.move_cursor(this.row_step(), true, cx)))
            .on_action(cx.listener(|this, _: &ExtendPrev, _, cx| this.move_cursor(-this.row_step(), true, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| match this.view_mode {
                ViewMode::Grid => this.move_cursor(-this.side_step(), false, cx),
                ViewMode::Columns => this.up(cx),
                ViewMode::Details => {}
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| match this.view_mode {
                ViewMode::Grid => this.move_cursor(this.side_step(), false, cx),
                ViewMode::Columns => {
                    if let Some(ei) = this.cursor_entry().filter(|&ei| this.entries[ei as usize].is_dir()) {
                        this.open_entry(ei, cx);
                    }
                }
                ViewMode::Details => {}
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.move_cursor(20, false, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.move_cursor(-20, false, cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.move_cursor(isize::MIN / 2, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.move_cursor(isize::MAX / 2, false, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.selected = this.visible.iter().copied().collect();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleAllGroups, _, cx| this.toggle_all_groups(cx)))
            .on_action(cx.listener(|this, _: &InvertSelection, _, cx| {
                let next: HashSet<u32> = this.visible.iter().copied().filter(|i| !this.selected.contains(i)).collect();
                this.selected = next;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSelection, _, cx| {
                if let Some(ei) = this.cursor.and_then(|c| this.visible.get(c)).copied() {
                    if !this.selected.remove(&ei) {
                        this.selected.insert(ei);
                    }
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, window, cx| {
                if QUICK_LOOK_OPEN.load(Ordering::Relaxed) {
                    cx.emit(FolderEvent::QuickLook);
                    return;
                }
                if this.renaming.is_some() {
                    this.cancel_rename(window, cx);
                    return;
                }
                this.selected.clear();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                if this.renaming.is_some() {
                    this.commit_rename(window, cx)
                } else {
                    this.open_selected(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, _, cx| {
                // the selected folders (a handful at most), or this folder with none selected
                let dirs: Vec<PathBuf> = this.selection_with_kind().into_iter().filter(|(_, d)| *d).map(|(p, _)| p).take(8).collect();
                if dirs.is_empty() && this.selected.is_empty() {
                    cx.emit(FolderEvent::OpenInNewWindow(this.location().clone()));
                }
                for d in dirs {
                    cx.emit(FolderEvent::OpenInNewWindow(Location::Path(d)));
                }
            }))
            .on_action(cx.listener(|this, _: &Properties, _, cx| this.properties(cx)))
            .on_action(cx.listener(|this, _: &OpenWith, _, _| this.open_with()))
            .on_action(cx.listener(|this, _: &CopyItems, window, cx| this.copy_to_clipboard(rf_win::fileops::Transfer::Copy, window, cx)))
            .on_action(cx.listener(|this, _: &CutItems, window, cx| this.copy_to_clipboard(rf_win::fileops::Transfer::Move, window, cx)))
            .on_action(cx.listener(|this, _: &PasteItems, window, cx| this.paste(window, cx)))
            .on_action(cx.listener(|this, _: &PasteIntoSelected, window, cx| this.paste_into_selected(window, cx)))
            .on_action(cx.listener(|this, _: &PasteShortcut, window, cx| this.paste_shortcuts(window, cx)))
            .on_action(cx.listener(|this, _: &DuplicateItems, _, cx| {
                // (in the rename box the chord is AltGr+D on some keyboards, a letter: the box has it)
                if this.renaming.is_some() {
                    cx.propagate();
                } else {
                    this.duplicate_selected(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyToOtherPane, _, cx| this.to_other_pane(rf_win::fileops::Transfer::Copy, cx)))
            .on_action(cx.listener(|this, _: &MoveToOtherPane, _, cx| this.to_other_pane(rf_win::fileops::Transfer::Move, cx)))
            .on_action(cx.listener(|this, _: &DeleteItems, w, cx| {
                if !this.bin_delete(w, cx) {
                    this.delete_selected(false, w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &DeletePermanently, w, cx| {
                if !this.bin_delete(w, cx) {
                    this.delete_selected(true, w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &RenameItem, window, cx| {
                if this.selected.len() > 1 {
                    cx.emit(FolderEvent::BulkRename(this.selection_paths()));
                } else {
                    this.begin_rename(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NewFolder, _, cx| this.create_new(true, cx)))
            .on_action(cx.listener(|this, _: &NewTextFile, _, cx| this.create_new(false, cx)))
            .on_action(cx.listener(|this, _: &DownloadFromLink, window, cx| this.download_from_link(window, cx)))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_path(cx)))
            .on_action(cx.listener(|this, _: &CopyPathQuoted, _, cx| this.copy_path_with(true, cx)))
            .on_action(cx.listener(|this, _: &RotateRight, window, cx| this.rotate_selection(true, window, cx)))
            .on_action(cx.listener(|this, _: &RotateLeft, window, cx| this.rotate_selection(false, window, cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if this.renaming.is_none() {
                    crate::undo::redo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1, cx)))
            .on_action(cx.listener(|this, _: &AddToShelf, _, cx| {
                let paths = this.selection_paths();
                if !paths.is_empty() {
                    cx.emit(FolderEvent::AddToShelf(paths));
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(-1, cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if this.renaming.is_none() {
                    crate::undo::undo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowContextMenu, window, cx| {
                let bg = this.selected.is_empty();
                this.show_context_menu(bg, window, cx)
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_typeahead(ev, cx)))
            .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| this.on_key_up(ev, cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.back(cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.forward(cx)),
            )
            .child(content)
    }
}

#[allow(dead_code)]
fn _assert_category_used(_: Category) {}

/// `RUSTYFILES_TRACE_LIVE=1`: log listing and live-update timings (benchmarks).
fn trace_live() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTYFILES_TRACE_LIVE").is_some())
}

/// What the first click of a possible double click was made on: the listing it was made in (`FolderView::load_serial`) and the
/// entry's name, `None` for the empty area.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FirstClick {
    load: u64,
    name: Option<Arc<str>>,
}

struct RowPress {
    origin: Point<Pixels>,
    open_on_release: bool,
}

/// Whether a click that Windows counts as the second of a double click is one HERE: its first click was made in this same listing
/// and on the same thing (the entry `name`, or the empty area for `None`).
///
/// WHY (a user on Discord, 2026-11-14: "when you click on one of the drive really fast like usual explorer thing the folders doesn't
/// load"). A drive or a folder tile of the Home page, a place in the sidebar, a part of the address opens on ONE click. A user who
/// double-clicks it, as in Explorer, sent a second click, and that click landed on whatever the new listing had under the pointer by
/// then: Windows still called it a double click, so the folder there was opened too (a double click on drive C: ended inside
/// `C:\MSI`), or, on the empty area with "double-click an empty area to go up" on, the drive was left again. Windows counts clicks by
/// time and place alone; what was under them is ours to know.
/// The list's and the Settings page's scrollbars: shown while scrolling, or all the time when Windows is set to always show
/// scrollbars (Windows 10's "Automatically hide scroll bars" off).
pub(crate) fn scrollbar_mode() -> ScrollbarMode {
    if rf_win::scrollbars::always_shown() { ScrollbarMode::Always } else { ScrollbarMode::Scrolling }
}

/// The grid's name of a right-to-left name in at most two lines (issue 115): GPUI wraps such a run after every letter, so the name is
/// split here, at a space, by measured width; each line is then drawn unwrapped (the last one ends in "…" when it is still too long).
/// One line when it fits or has no space to split at. A last line that is still too long is cut here too, with the "…" at the END
/// of its text: drawn right to left, that is its LEFT side, where a right-to-left reader expects it (issue 115: the text renderer's own
/// cut put it on the right).
fn rtl_grid_lines(name: &str, avail: Pixels, window: &Window) -> Vec<String> {
    let style = window.text_style();
    let size = rems(0.75).to_pixels(window.rem_size());
    let width = |s: &str| {
        let text: SharedString = s.to_string().into();
        let run = style.to_run(text.len());
        window.text_system().shape_line(text, size, &[run], None).width
    };
    if width(name) <= avail {
        return vec![name.to_string()];
    }
    let words: Vec<&str> = name.split(' ').collect();
    let mut take = 0;
    for i in 1..words.len() {
        if width(&words[..i].join(" ")) > avail {
            break;
        }
        take = i;
    }
    if take == 0 {
        return vec![cut_to_width(name, avail, &width)];
    }
    vec![words[..take].join(" "), cut_to_width(&words[take..].join(" "), avail, &width)]
}

/// `text` as it is when it fits in `avail`, else shortened at its end (in reading order) to what fits with "…" after it.
fn cut_to_width(text: &str, avail: Pixels, width: &dyn Fn(&str) -> Pixels) -> String {
    if width(text) <= avail {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        // (a right-to-left mark after the "…": the text renderer lays the line out left to right, and a "…" with nothing strong
        // after it took that direction and stood on the right; between two right-to-left letters it goes with them, to the left)
        let cut: String = chars.iter().collect::<String>().trim_end().to_string() + "…\u{200F}";
        if width(&cut) <= avail {
            return cut;
        }
    }
    "…".to_string()
}

/// The key in the layouts file under which "the same layout in all folders" keeps its one layout (`<` cannot start a real path).
const SHARED_LAYOUT: &str = "<all folders>";
/// The layouts' key of the one layout all searches share.
const SEARCH_LAYOUT: &str = "<search results>";

/// How long the pointer rests on an item before "Select on hover" selects it: the system's hover time (Control Panel › Mouse;
/// 400 ms out of the box), as Explorer's "point to select" waits.
fn hover_time() -> std::time::Duration {
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETMOUSEHOVERTIME, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut ms: u32 = 0;
    let ok = unsafe { SystemParametersInfoW(SPI_GETMOUSEHOVERTIME, 0, Some(&mut ms as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }.is_ok();
    std::time::Duration::from_millis(if ok && ms > 0 { ms.min(5000) as u64 } else { 400 })
}

fn completes_double(first: Option<&FirstClick>, load: u64, name: Option<&str>) -> bool {
    first.is_some_and(|f| f.load == load && f.name.as_deref() == name)
}

#[cfg(test)]
mod double_click_tests {
    use super::{FirstClick, completes_double};

    fn first(load: u64, name: Option<&str>) -> FirstClick {
        FirstClick { load, name: name.map(Into::into) }
    }

    #[test]
    fn a_double_click_is_two_clicks_on_the_same_thing_in_the_same_listing() {
        let on_msi = first(7, Some("MSI"));
        assert!(completes_double(Some(&on_msi), 7, Some("MSI")));
        // the first click was made before this listing was shown (it opened the drive from the Home page)
        assert!(!completes_double(Some(&on_msi), 8, Some("MSI")));
        assert!(!completes_double(None, 8, Some("MSI")), "no first click here at all");
        // the same listing, another entry (the list moved under the pointer between the clicks)
        assert!(!completes_double(Some(&on_msi), 7, Some("Program Files")));
        // the empty area: both clicks on it, in this listing
        let on_empty = first(7, None);
        assert!(completes_double(Some(&on_empty), 7, None));
        assert!(!completes_double(Some(&on_empty), 8, None), "the folder was just opened: it is not left again");
        assert!(!completes_double(Some(&on_msi), 7, None) && !completes_double(Some(&on_empty), 7, Some("MSI")), "a row and the empty area are not the same thing");
    }
}

#[cfg(test)]
mod known_kind_tests {
    use super::{KnownKind, known_kind};
    use rf_win::known::KnownFolder;

    #[test]
    fn pictures_and_downloads_are_recognised() {
        let (Some(pics), Some(dl)) = (KnownFolder::Pictures.path(), KnownFolder::Downloads.path()) else { return };
        assert_eq!(known_kind(&pics), Some(KnownKind::Pictures));
        assert_eq!(known_kind(&pics.join("Screenshots")), Some(KnownKind::Pictures));
        assert_eq!(known_kind(&dl), Some(KnownKind::Downloads));
        // only Downloads itself gets date groups, not the folders inside it
        assert_eq!(known_kind(&dl.join("project")), None);
        // a sibling whose name starts the same is not inside Pictures
        let sibling = std::path::PathBuf::from(format!("{} old", pics.display()));
        assert_eq!(known_kind(&sibling), None);
    }
}

/// Name-index hits to rows: each path is looked up (size, dates, attributes) on 8 threads, 2 000
/// at a time, and sent in order; paths gone since they were indexed are left out. Returns how
/// many were sent.
fn send_index_hits(paths: &[PathBuf], tx: &mpsc::UnboundedSender<Msg>, cancel: &AtomicBool) -> usize {
    use std::os::windows::fs::MetadataExt;
    let ms = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    let row = |p: &PathBuf| -> Option<(Entry, Arc<std::path::Path>)> {
        let m = std::fs::symlink_metadata(p).ok()?;
        let kind = if m.is_dir() { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File };
        let name = p.file_name()?.to_string_lossy().into_owned();
        let parent: Arc<std::path::Path> = p.parent().unwrap_or(p).into();
        Some((Entry::new(&name, kind, if m.is_dir() { 0 } else { m.len() }, ms(m.modified()), ms(m.created()), m.file_attributes()), parent))
    };
    let mut sent = 0usize;
    for batch in paths.chunks(2000) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let per = batch.len().div_ceil(8).max(1);
        let parts: Vec<Vec<(Entry, Arc<std::path::Path>)>> = std::thread::scope(|s| {
            let handles: Vec<_> = batch.chunks(per).map(|c| s.spawn(move || c.iter().filter_map(row).collect::<Vec<_>>())).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        let (es, ps): (Vec<Entry>, Vec<Arc<std::path::Path>>) = parts.into_iter().flatten().unzip();
        sent += es.len();
        let _ = tx.unbounded_send(Msg::Hits(es, ps));
    }
    sent
}

#[cfg(test)]
mod rename_name_tests {
    use super::shown_new_name;

    /// The name a row shows while its rename is under way is what an entry of that name would show: whole, or without its
    /// extension when extensions are hidden (a folder and a name that is all extension keep theirs).
    #[test]
    fn the_new_name_of_a_rename_is_shown_as_an_entry_of_that_name_would_be() {
        assert_eq!(shown_new_name("report.pdf", false, true), "report.pdf");
        assert_eq!(shown_new_name("report.pdf", false, false), "report");
        assert_eq!(shown_new_name("archive.tar.gz", false, false), "archive.tar");
        assert_eq!(shown_new_name(".gitignore", false, false), ".gitignore");
        assert_eq!(shown_new_name("folder.v2", true, false), "folder.v2");
        assert_eq!(shown_new_name("noext", false, false), "noext");
        assert_eq!(shown_new_name("ends.", false, false), "ends.");
    }
}

/// An item's icon with Explorer's link arrow at its lower left when the item is a symbolic link or a junction (issue 50): a small
/// light square with a blue arrow, as Explorer draws it, so it reads the same in a light and a dark list. The icon keeps its place
/// and size; a plain item's icon is returned as it is.
/// The square another program's overlay picture is drawn in, at the bottom left of an icon drawn `size` wide.
fn overlay_box(size: Pixels) -> Pixels {
    size.min(px(48.))
}

/// `icon` with the marks drawn over it: another program's overlay (`overlay`, icon-sized with its mark in a corner, as Explorer draws
/// it) and the arrow of a symbolic link or junction (`link`).
/// An unfinished download's file (`.hdpart`) shows how far it is: a thin molten bar under its icon.
fn download_marked(icon: AnyElement, frac: Option<f32>) -> AnyElement {
    let Some(f) = frac else { return icon };
    div()
        .relative()
        .flex_shrink_0()
        .child(icon)
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(-3.))
                .h(px(2.))
                .rounded_full()
                .bg(gpui::rgba(0x8080_8055))
                .child(div().h_full().rounded_full().bg(crate::downloads_view::ember()).w(relative(f.clamp(0.02, 1.)))),
        )
        .into_any_element()
}

fn link_marked(icon: AnyElement, size: Pixels, link: bool, overlay: Option<Arc<gpui::RenderImage>>) -> AnyElement {
    if !link && overlay.is_none() {
        return icon;
    }
    let s = (size * 0.3).max(px(8.)).min(px(22.));
    div()
        .relative()
        .flex_shrink_0()
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .child(icon)
        // (in the corner of a big tile at most a 48 px icon's overlay: drawn the tile's size, a mark meant for a small corner covered
        // half the folder)
        .when_some(overlay, |d, im| d.child(img(im).absolute().left_0().bottom_0().size(overlay_box(size))))
        .when(link, |d| d.child(
            div()
                .absolute()
                .left(px(-1.))
                .bottom(px(-1.))
                .size(s)
                .rounded(px(2.))
                .bg(gpui::rgb(0xFFFFFF))
                .border_1()
                .border_color(gpui::rgba(0x0000_0040))
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(L::ArrowUpRight).size(s * 0.85).text_color(gpui::rgb(0x1A73E8))),
        ))
        .into_any_element()
}

impl FolderView {
    /// Every shown item selected (the toolbar's "…" menu; Ctrl+A does the same through its action).
    pub(crate) fn select_all_items(&mut self, cx: &mut Context<Self>) {
        self.selected = self.visible.iter().copied().collect();
        cx.notify();
    }

    /// Nothing selected; the cursor stays where it is.
    pub(crate) fn select_no_items(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        cx.notify();
    }

    /// The shown items that were not selected become the selection.
    pub(crate) fn invert_selected_items(&mut self, cx: &mut Context<Self>) {
        let next: HashSet<u32> = self.visible.iter().copied().filter(|i| !self.selected.contains(i)).collect();
        self.selected = next;
        cx.notify();
    }
}

/// Use the rendered path, so a refresh during a drag cannot retarget it by shifting row indices.
fn file_drop_target(row: Stateful<Div>, target: PathBuf, is_dir: bool, p: &Palette) -> Stateful<Div> {
    if !is_dir && !crate::program_launch::accepts_program(&target) {
        return row;
    }
    let (color, border) = (p.selection, p.accent);
    let hover_target = target.clone();
    row.drag_over::<ExternalPaths>(move |s, paths, _, _| {
        if valid_item_drop(paths.paths(), &hover_target, is_dir) { s.bg(color).border_1().border_color(border) } else { s }
    })
    .on_drag_move::<ExternalPaths>({
        let target = target.clone();
        move |ev, _, cx| {
            if !ev.bounds.contains(&ev.event.position) {
                return;
            }
            if is_dir {
                show_drop_effect(ev.drag(cx).paths(), &target);
            } else {
                gpui_windows::dnd_hint::set(gpui_windows::dnd_hint::COPY);
            }
        }
    })
    .on_drop(move |paths: &ExternalPaths, window, cx| {
        if !valid_item_drop(paths.paths(), &target, is_dir) {
            return;
        }
        if is_dir {
            crate::workspace::drop_files(paths, target.clone(), cx);
        } else {
            crate::ops::drop_on_program(target.clone(), paths.paths().to_vec(), window, cx);
        }
    })
}

fn valid_item_drop(sources: &[PathBuf], target: &std::path::Path, is_dir: bool) -> bool {
    !sources.is_empty() && if is_dir {
        !rf_win::dnd::is_invalid_drop(sources, target)
    } else {
        !sources.iter().any(|p| p.as_os_str().eq_ignore_ascii_case(target.as_os_str()))
    }
}

#[cfg(test)]
mod file_drop_tests {
    use super::valid_item_drop;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_child_folder_or_program_accepts_a_file_but_never_itself() {
        let file = PathBuf::from(r"C:\files\report.txt");
        let folder = Path::new(r"C:\files\sub");
        let program = Path::new(r"C:\files\sign.EXE");
        assert!(valid_item_drop(std::slice::from_ref(&file), folder, true));
        assert!(valid_item_drop(&[file], program, false));
        assert!(!valid_item_drop(&[folder.to_path_buf()], folder, true));
        assert!(!valid_item_drop(&[PathBuf::from(r"c:\files\SIGN.exe")], program, false));
    }
}

/// Tells Windows which effect to show under the pointer ("Move to", "Copy to") for files dragged over a drop target whose folder is
/// `dir`: what the drop will do there (`rf_win::dnd::default_transfer`: the same drive moves, another drive copies). Every drop target
/// says it for its own folder (issue 54: dragging from W: onto the tree's D: row said "Move" and copied: the tree said nothing, and
/// the list's last word, W: onto W:, stayed).
pub(crate) fn show_drop_effect(paths: &[PathBuf], dir: &std::path::Path) {
    let t = rf_win::dnd::default_transfer(paths, dir, false, false);
    log::debug!("drag over {}: {t:?} ({} item(s), the first {:?})", dir.display(), paths.len(), paths.first());
    gpui_windows::dnd_hint::set(match t {
        rf_win::fileops::Transfer::Move => gpui_windows::dnd_hint::MOVE,
        rf_win::fileops::Transfer::Copy => gpui_windows::dnd_hint::COPY,
    });
}

/// [`show_drop_effect`] as a drag-move listener of a drop target, for the moves over it.
pub(crate) fn drop_effect_over(dir: PathBuf) -> impl Fn(&DragMoveEvent<ExternalPaths>, &mut Window, &mut App) + 'static {
    move |ev, _, cx| {
        if ev.bounds.contains(&ev.event.position) {
            show_drop_effect(ev.drag(cx).paths(), &dir);
        }
    }
}

/// The wheel's travel (in pixels; one notch counts as this much) that makes one size step with Ctrl held.
const ZOOM_STEP: f32 = 40.0;

/// How long a Windows Search query may give nothing before the disk is walked instead (issue 67). A query that answers at all is
/// waited for to its end.
const WSEARCH_SILENCE: Duration = Duration::from_secs(8);

/// Runs `f` on a thread of its own and waits for its answer at most `limit`; `None` when it did not come in time (the thread is
/// left to end by itself).
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("rf-within".into()).spawn(move || {
        let _ = tx.send(f());
    }).ok()?;
    rx.recv_timeout(limit).ok()
}

#[cfg(test)]
mod grid_units_tests {
    use super::{GridUnit, Row, grid_units};

    fn rows(groups: &[(&str, usize)]) -> Vec<Row> {
        let mut out = Vec::new();
        let mut vi = 0;
        for (label, n) in groups {
            out.push(Row::Header((*label).to_string().into(), *n));
            for _ in 0..*n {
                out.push(Row::Item(vi));
                vi += 1;
            }
        }
        out
    }

    #[test]
    fn each_group_starts_a_row_and_a_row_spans_its_slices() {
        // 5 items then 2, three tiles a row, two slices a row
        let (units, first) = grid_units(&rows(&[("Today", 5), ("Earlier", 2)]), 3, 2);
        let tiles = |start, len, part| GridUnit::Tiles { start, len, part };
        assert_eq!(
            units,
            vec![
                GridUnit::Header("Today".into(), 5),
                tiles(0, 3, 0),
                tiles(0, 3, 1),
                tiles(3, 2, 0),
                tiles(3, 2, 1),
                GridUnit::Header("Earlier".into(), 2),
                tiles(5, 2, 0),
                tiles(5, 2, 1),
            ]
        );
        assert_eq!(first, [1, 1, 1, 3, 3, 6, 6], "each item's row starts at its first slice");
    }

    #[test]
    fn a_folded_group_keeps_its_header_alone() {
        let mut r = rows(&[("A", 2)]);
        r.push(Row::Header("Folded".to_string().into(), 4));
        let (units, first) = grid_units(&r, 4, 3);
        assert_eq!(units.len(), 1 + 3 + 1);
        assert_eq!(units[4], GridUnit::Header("Folded".into(), 4));
        assert_eq!(first, [1, 1]);
    }
}
