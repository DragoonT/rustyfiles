//! The folder tree section of the sidebar ("Klasörler", setting `sidebar_tree`, off by default).
//!
//! The model is `rf_core::navtree::NavTree` (nodes, the open set, the rows, the follow rule, the keys); this file is the thin
//! view on top of it: it lists folders off the UI thread when the model asks, watches the open folders, draws only the rows that
//! are on the screen, and turns clicks, drops and keys into events for the window (navigate, open in a tab, the folder menu).
//!
//! This is a child module of `workspace` (see the `#[path]` line there), so the few hooks the window needs are written here as
//! methods of `Workspace` and the window file stays small.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc;
use rf_core::navtree::{Change, DirItem, ListFailure, LoadRequest, NavTree, NodeId, RootKind, RootSpec, Row, RowKind, SLOW_LOAD_MS, TreeFilter, TreeKey, norm, scroll_goal, visible_range};
use rf_win::DriveKind;
use rf_win::listing::ListError;
use rf_win::watch::DirWatch;

use super::*;

/// Height of one tree row. Fixed, so that the rows on the screen can be worked out from the scroll position.
const ROW_H: f32 = 30.;
/// Pixels of indent per level: every level, however deep.
const INDENT: f32 = 16.;
/// Past this level a row's tooltip has its whole name (the deep rows are the ones whose names are cut short).
const TOOLTIP_PAST_LEVEL: u16 = 6;
/// The room a row's name keeps when the tree is moved sideways for the row.
const NAME_ROOM: f32 = 96.;
/// What stands between a row's indent and its name (the row's padding, the chevron, the icon and their gaps), and after the name.
const ROW_LEAD: f32 = 8. + 16. + 4. + 16. + 4.;
const ROW_TAIL: f32 = 8.;
/// How long the tree's move sideways takes.
const SLIDE_MS: f32 = 180.;

/// A row's indent for its depth: a folder's children stand to the right of it at every depth.
///
/// HISTORY (issue 32, with a picture beside Explorer's tree: "the indentation is just at the same level as the parent folder ...
/// usually deeper levels"). Until 0.8.1 every row past the sixth level stood at the SAME indent, so that names kept room in a
/// narrow sidebar: what was in what could not be read. 0.8.2 gave six more levels half a step each. Now every level has the whole
/// step, and a tree too deep for the sidebar's width is moved sideways instead, as Explorer's is ([`sideways_goal`]).
fn indent_of(depth: u16) -> f32 {
    INDENT * f32::from(depth)
}

/// How far the tree stands moved to the left so that a row with this indent can be read in a tree `width` wide: its name keeps
/// [`NAME_ROOM`] and its chevron is not cut off. The least movement from where the tree stands now (`cur`), as the scroll to a row
/// is; where both cannot be had (a very narrow sidebar) the row's start wins. 0: not moved. A tree that is not measured yet stays.
fn sideways_goal(cur: f32, indent: f32, width: f32) -> f32 {
    if width <= 1.0 {
        return cur.max(0.0);
    }
    let most = indent.max(0.0);
    let least = (indent + ROW_LEAD + NAME_ROOM + ROW_TAIL - width).clamp(0.0, most);
    cur.clamp(least, most)
}

/// The furthest the tree may stand moved: what its deepest row needs. A move by hand is held to it, and a tree whose deep branches
/// were closed goes back to it.
fn sideways_most(deepest: u16, width: f32) -> f32 {
    sideways_goal(0.0, indent_of(deepest), width.max(0.0)) * f32::from(u8::from(width > 1.0))
}

/// The tree's place sideways: how far it is moved to the left, and the slide that takes it there.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Slide {
    from: f32,
    to: f32,
    since: Option<Instant>,
}

impl Slide {
    const REST: Slide = Slide { from: 0.0, to: 0.0, since: None };

    /// Where the tree is now, and whether it is still on its way.
    fn at(&self, now: Instant) -> (f32, bool) {
        let Some(since) = self.since else { return (self.to, false) };
        let x = now.saturating_duration_since(since).as_secs_f32() * 1000.0 / SLIDE_MS;
        if x >= 1.0 { (self.to, false) } else { (self.from + (self.to - self.from) * rf_win::dock::spring::curve_with(x, 0.0), true) }
    }

    /// Sends it to `to`: from where it is at this moment (a slide that is turned around does not jump), or at once.
    fn go(&mut self, to: f32, now: Instant, animated: bool) {
        if (to - self.to).abs() < 0.5 {
            return;
        }
        *self = Slide { from: self.at(now).0, to, since: animated.then_some(now) };
    }
}
/// How long a slow listing's spinner pulses (three pulses), then it stands still: a listing that hangs (an offline share) must not
/// keep the window drawing.
const WAIT_PULSE_MS: u64 = 2700;
/// Rows drawn above and below the window of the screen, so that a quick scroll does not show holes.
const OVERSCAN: usize = 6;
/// Rows drawn before the scroll area has been measured (the first frame).
const FIRST_FRAME_ROWS: usize = 40;

/// What the window has to do for the tree.
pub enum TreeEvent {
    /// A click, Enter: show this folder in the active pane. The focus stays in the tree.
    Navigate(PathBuf),
    /// Ctrl+click, middle click: open it in a new tab.
    OpenTab(PathBuf),
    /// Right click: the same menu a sidebar folder gets (Shift held: Windows' own menu, as on the sidebar's rows).
    Menu(PathBuf),
    /// The menu key, Shift+F10: the sidebar's folder menu next to the row (the point), never Windows' classic menu.
    KeyMenu(PathBuf, Point<Pixels>),
    /// Escape, Tab: give the focus back to the file list.
    Leave,
}

enum Msg {
    Changes(PathBuf, Vec<Change>),
    Rescan(PathBuf),
}

/// What is the same for every row of one frame.
#[derive(Clone, Copy)]
struct Frame {
    /// The row that stands for the open folder, and whether it is the folder's own row.
    mark: Option<(usize, bool)>,
    focused: bool,
    now: u64,
    /// How far the tree is moved to the left in this frame (see `sideways_goal`).
    aside: f32,
}

#[derive(Clone, Copy)]
struct Goal {
    row: usize,
    /// After the open folder changed: the row goes to the upper third unless it is on the screen already.
    /// After a key: the least movement that shows it.
    centre: bool,
    tries: u8,
}

pub struct SidebarTree {
    model: NavTree,
    places: Entity<Places>,
    /// The window's icons: a customised folder's own icon on its row.
    images: Entity<crate::image_cache::ImageCache>,
    /// The sidebar's scroll area: the tree is part of its content and scrolls with it.
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// Where the top of the tree is in the scroll area's content, in pixels (measured at every paint).
    tree_top: Rc<Cell<f32>>,
    /// The tree's width (measured with `tree_top`).
    tree_w: Rc<Cell<f32>>,
    /// How far the tree is moved to the left for a deep row (see `sideways_goal`).
    slide: Slide,
    /// What the tree was last moved for: the open folder's row and the keyboard's row (node and depth), and the width it had.
    slid_marked: Option<(NodeId, u16)>,
    slid_cursor: Option<(NodeId, u16)>,
    slid_width: f32,
    /// The deepest level among the rows, with the number of rows it was counted for.
    deepest: (usize, u16),
    epoch: Instant,
    goal: Option<Goal>,
    last_roots: Vec<(PathBuf, String, RootKind)>,
    /// Another row of the sidebar (a pinned folder, a drive) already shows the open folder as selected: the tree then draws no
    /// selection and no mark of its own, so the sidebar never has two selected rows (the user's report).
    shown_elsewhere: bool,
    /// Over glass the tree's selected row is the sidebar's glass lens, like the selected row of the sidebar's other sections (the
    /// engine draws the pill behind the scene; the row carries a probe that says where it is): the lens's key and the window, when
    /// this frame's sidebar gave the lens to the tree. `None`: the Fluent fill and accent pill.
    lens: Option<(u64, isize)>,
    /// The selected row was among the rows built at the last paint (the rows are virtualised: a selected row that is scrolled far
    /// out of view is not built, carries no probe, and must not hold the lens, which would stay where it last was).
    selected_in_view: bool,
    /// The user's profile folder, asked once (the first root).
    home: Option<PathBuf>,
    /// The open folder and the follow setting the model was last told about.
    followed: Option<(Option<PathBuf>, bool)>,
    watchers: HashMap<String, DirWatch>,
    pending_watch: HashSet<String>,
    wanted: HashSet<String>,
    tx: mpsc::UnboundedSender<Msg>,
    _rx_task: gpui_kit::Task<()>,
    _subs: Vec<gpui_kit::Subscription>,
}

impl EventEmitter<TreeEvent> for SidebarTree {}

/// The tree's owner in the window: the view (made when the setting is on), the sidebar's scroll handle, and the rail's hover
/// peek flag. A window without the setting holds nothing but this empty value.
#[derive(Default)]
pub struct Holder {
    view: Option<Entity<SidebarTree>>,
    scroll: ScrollHandle,
    /// The rail's hover peek draws a second sidebar; the rail itself shows no tree and neither does its peek.
    in_peek: bool,
    sub: Option<gpui_kit::Subscription>,
}

/// The folders of a folder, on a worker thread.
fn list_folders(path: &Path) -> Result<Vec<DirItem>, ListFailure> {
    let mut out = Vec::new();
    let r = rf_win::list_dir(path, 4096, &|| false, &mut |batch| {
        out.extend(batch.iter().filter(|e| e.is_dir()).map(dir_item));
    });
    match r {
        Ok(_) => Ok(out),
        Err(ListError::NotFound) => Err(ListFailure::NotFound),
        Err(e) => Err(ListFailure::Other(crate::i18n::td(&e.to_string()))),
    }
}

/// What the watcher's names are now: a folder (with its flags) or nothing the tree shows (gone, or a file). `Err`: it can't be told
/// (the folder itself went, no access...) and the folder is listed again instead.
fn read_changes(dir: &Path, names: Vec<String>) -> Result<Vec<Change>, ListError> {
    names
        .into_iter()
        .map(|n| {
            rf_win::stat_entry(dir, &n).map(|e| match e {
                Some(e) if e.is_dir() => Change { name: e.name.to_string(), item: Some(dir_item(&e)) },
                _ => Change { name: n, item: None },
            })
        })
        .collect()
}

fn dir_item(e: &rf_core::Entry) -> DirItem {
    DirItem {
        name: e.name.to_string(),
        label: e.label.as_deref().map(str::to_string),
        hidden: e.is_hidden(),
        system: e.is_system(),
        own_icon: rf_win::thumbs::folder_has_own_icon(e.attrs),
    }
}

fn root_kind(k: DriveKind) -> RootKind {
    match k {
        DriveKind::Removable => RootKind::Removable,
        DriveKind::Network => RootKind::Network,
        DriveKind::Optical => RootKind::Optical,
        _ => RootKind::Fixed,
    }
}

impl SidebarTree {
    fn new(places: Entity<Places>, images: Entity<crate::image_cache::ImageCache>, scroll: ScrollHandle, cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let task = cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                if this.update(cx, |t, cx| t.on_msg(msg, cx)).is_err() {
                    break;
                }
            }
        });
        // drives come and go: the roots follow the sidebar's own list of places
        let sub = cx.observe(&places, |t, _, cx| t.refresh_roots(cx));
        let tags_sub = cx.observe_global::<crate::tags::Tags>(|_, cx| cx.notify());
        let mut this = Self {
            model: NavTree::new(),
            places,
            images,
            scroll,
            focus: cx.focus_handle(),
            tree_top: Rc::new(Cell::new(0.)),
            tree_w: Rc::new(Cell::new(0.)),
            slide: Slide::REST,
            slid_marked: None,
            slid_cursor: None,
            slid_width: 0.,
            deepest: (0, 0),
            epoch: Instant::now(),
            goal: None,
            last_roots: Vec::new(),
            shown_elsewhere: false,
            lens: None,
            selected_in_view: true,
            home: dirs::home_dir(),
            followed: None,
            watchers: HashMap::new(),
            pending_watch: HashSet::new(),
            wanted: HashSet::new(),
            tx,
            _rx_task: task,
            _subs: vec![sub, tags_sub],
        };
        this.refresh_roots(cx);
        this
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// The profile folder first, then the drives, as the sidebar's places have them.
    fn refresh_roots(&mut self, cx: &mut Context<Self>) {
        let places = self.places.read(cx);
        if !places.loaded {
            return;
        }
        let mut specs = Vec::new();
        if let Some(home) = self.home.clone() {
            specs.push(RootSpec { label: rf_core::history::display_name(&home), path: home, kind: RootKind::Profile });
        }
        for d in &places.drives {
            specs.push(RootSpec { path: d.root.clone(), label: crate::places::drive_name(d), kind: root_kind(d.kind) });
        }
        let key: Vec<(PathBuf, String, RootKind)> = specs.iter().map(|s| (s.path.clone(), s.label.clone(), s.kind)).collect();
        if key == self.last_roots {
            return;
        }
        self.last_roots = key;
        let reqs = self.model.set_roots(specs, self.now());
        self.start_loads(reqs, cx);
        self.after_change(cx);
    }

    /// Called by the window at every frame the section is drawn: the settings that shape the tree and the open folder.
    pub fn sync(&mut self, cur: Option<&Path>, shown_elsewhere: bool, lens: Option<(u64, isize)>, cx: &mut Context<Self>) {
        if self.shown_elsewhere != shown_elsewhere || self.lens != lens {
            self.shown_elsewhere = shown_elsewhere;
            self.lens = lens;
            cx.notify();
        }
        let s = cx.global::<crate::settings::Settings>();
        let filter = TreeFilter { show_hidden: s.show_hidden, show_system: s.show_system_files, dot_hidden: !s.show_dot_files };
        let follow = s.sidebar_tree_follow;
        let mut changed = self.model.set_filter(filter);
        let key = (cur.map(Path::to_path_buf), follow);
        if self.followed.as_ref() != Some(&key) {
            self.followed = Some(key);
            let reqs = self.model.follow(cur, self.now(), follow);
            self.start_loads(reqs, cx);
            changed = true;
        }
        if changed {
            self.after_change(cx);
        }
    }

    /// Whether the open folder's own row is in the tree now (the row that is drawn selected).
    pub fn has_selected_row(&self) -> bool {
        self.selected_in_view && self.model.marked_row().is_some_and(|(_, own)| own)
    }

    fn after_change(&mut self, cx: &mut Context<Self>) {
        self.sync_watchers(cx);
        cx.notify();
    }

    fn start_loads(&mut self, reqs: Vec<LoadRequest>, cx: &mut Context<Self>) {
        for req in reqs {
            let path = req.path.clone();
            let work = cx.background_spawn(async move { list_folders(&path) });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |t, cx| t.finish_load(req, result, cx));
            })
            .detach();
        }
        if self.model.any_loading() {
            // wake up once the first listing has taken long enough to show the spinner
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(SLOW_LOAD_MS + 30)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    fn finish_load(&mut self, req: LoadRequest, result: Result<Vec<DirItem>, ListFailure>, cx: &mut Context<Self>) {
        let more = self.model.apply_listing(&req, result, self.now());
        self.start_loads(more, cx);
        self.after_change(cx);
    }

    fn on_msg(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Changes(dir, changes) => {
                if self.model.apply_changes(&dir, changes) {
                    self.after_change(cx);
                }
            }
            Msg::Rescan(dir) => {
                let reqs = self.model.rescan(&dir);
                self.start_loads(reqs, cx);
            }
        }
    }

    /// One watcher per open folder (never more than the model allows), none for closed ones.
    fn sync_watchers(&mut self, cx: &mut Context<Self>) {
        let want: HashMap<String, PathBuf> = self.model.watch_set().into_iter().map(|p| (norm(&p), p)).collect();
        self.watchers.retain(|k, _| want.contains_key(k));
        self.pending_watch.retain(|k| want.contains_key(k));
        self.wanted = want.keys().cloned().collect();
        for (key, path) in want {
            if self.watchers.contains_key(&key) || !self.pending_watch.insert(key.clone()) {
                continue;
            }
            let tx = self.tx.clone();
            // Opening the directory handle can block (an offline share): not on the UI thread.
            let setup = cx.background_spawn(async move {
                let dir = path.clone();
                rf_win::watch::watch_dir_changes(&path, Duration::from_millis(250), move |ch| {
                    // read on the watcher's own thread: which names are folders now
                    let msg = if ch.rescan || ch.names.is_empty() {
                        Msg::Rescan(dir.clone())
                    } else {
                        match read_changes(&dir, ch.names) {
                            Ok(c) => Msg::Changes(dir.clone(), c),
                            Err(_) => Msg::Rescan(dir.clone()),
                        }
                    };
                    let _ = tx.unbounded_send(msg);
                })
                .map_err(|e| log::warn!("tree watch {path:?}: {e}"))
                .ok()
            });
            cx.spawn(async move |this, cx| {
                let w = setup.await;
                let _ = this.update(cx, |t, _| {
                    t.pending_watch.remove(&key);
                    if let Some(w) = w
                        && t.wanted.contains(&key)
                    {
                        t.watchers.insert(key, w);
                    }
                });
            })
            .detach();
        }
    }

    // ---- the user ----------------------------------------------------------------------------

    fn follow_setting(&self, cx: &App) -> bool {
        cx.global::<crate::settings::Settings>().sidebar_tree_follow
    }

    fn click_row(&mut self, id: NodeId, ctrl: bool, cx: &mut Context<Self>) {
        let Some(path) = self.model.path_of(id).map(Path::to_path_buf) else { return };
        if ctrl {
            self.model.set_cursor(id);
            cx.emit(TreeEvent::OpenTab(path));
        } else {
            self.select(id, path, cx);
        }
        cx.notify();
    }

    /// The row is the selected one; the window navigates, and the follow step that comes with the new place finds it done.
    fn select(&mut self, id: NodeId, path: PathBuf, cx: &mut Context<Self>) {
        self.model.select_node(id);
        let follow = self.follow_setting(cx);
        self.followed = Some((Some(path.clone()), follow));
        cx.emit(TreeEvent::Navigate(path));
    }

    fn toggle(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let reqs = self.model.toggle(id, self.now());
        self.model.set_cursor(id);
        self.start_loads(reqs, cx);
        self.after_change(cx);
    }

    fn retry(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let reqs = self.model.retry(id, self.now());
        self.start_loads(reqs, cx);
        self.after_change(cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        if m.control || m.alt || m.platform {
            return;
        }
        if key == "menu" || (key == "f10" && m.shift) {
            if let Some(row) = self.model.cursor_row()
                && let Some(path) = self.model.rows().get(row).and_then(|r| self.model.path_of(r.id)).map(Path::to_path_buf)
            {
                cx.stop_propagation();
                cx.emit(TreeEvent::KeyMenu(path, self.row_anchor(row)));
            }
            return;
        }
        // Plus and minus open and close the folder the cursor is on, as in Explorer's tree (issue 38). With or without Shift: "+" is a
        // shifted key on most layouts. (Tab and Escape hand the keyboard to the file list, below.)
        let typed = ev.keystroke.key_char.as_deref();
        let plus_minus = match (key, typed) {
            ("+", _) | ("add", _) | (_, Some("+")) => Some(TreeKey::Open),
            ("-", _) | ("subtract", _) | (_, Some("-")) => Some(TreeKey::Close),
            // (the star opens everything below, a few levels deep: `TreeKey::OpenAll`)
            ("*", _) | ("multiply", _) | (_, Some("*")) => Some(TreeKey::OpenAll),
            _ => None,
        };
        if let Some(k) = plus_minus {
            cx.stop_propagation();
            self.apply_key(k, cx);
            return;
        }
        if m.shift {
            return;
        }
        let k = match key {
            "up" => TreeKey::Up,
            "down" => TreeKey::Down,
            "left" => TreeKey::Left,
            "right" => TreeKey::Right,
            "home" => TreeKey::Home,
            "end" => TreeKey::End,
            "enter" => TreeKey::Enter,
            "escape" | "tab" => {
                cx.stop_propagation();
                cx.emit(TreeEvent::Leave);
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.apply_key(k, cx);
    }

    /// The keyboard came to the tree with Tab (issue 38: "the selected item should be made visible (jump to item) or if no item is
    /// selected, select the first"): the keyboard's row, else the open folder's, else the first, is brought into view and is where
    /// the arrows start.
    pub(super) fn keyboard_entered(&mut self, cx: &mut Context<Self>) {
        let row = self
            .model
            .cursor_row()
            .or_else(|| self.model.marked_row().map(|(r, _)| r))
            .or_else(|| (!self.model.rows().is_empty()).then_some(0));
        let Some(row) = row else { return };
        if self.model.cursor_row().is_none()
            && let Some(id) = self.model.rows().get(row).map(|r| r.id)
        {
            self.model.set_cursor(id);
        }
        self.goal = Some(Goal { row, centre: false, tries: 2 });
        cx.notify();
    }

    fn apply_key(&mut self, k: TreeKey, cx: &mut Context<Self>) {
        let r = self.model.key(k, self.now());
        self.start_loads(r.loads, cx);
        if let Some(row) = r.cursor_row {
            self.goal = Some(Goal { row, centre: false, tries: 2 });
        }
        if let Some((id, path)) = r.navigate {
            self.select(id, path, cx);
        }
        if r.changed {
            self.after_change(cx);
        }
    }

    /// A sideways turn of the wheel (Shift and the wheel, a touchpad): the tree moves by hand, as far as its deepest row needs.
    /// Says whether the turn was the tree's to take: false where no row is deep enough for the tree to move at all, and the turn
    /// goes on to the sidebar (which takes a sideways turn as one up or down, as it always did).
    fn slide_by(&mut self, dx: f32, cx: &mut Context<Self>) -> bool {
        let most = sideways_most(self.deepest.1, self.tree_w.get());
        let to = (self.slide.to + dx).clamp(0.0, most);
        if (to - self.slide.to).abs() > 0.01 {
            self.slide = Slide { from: to, to, since: None };
            cx.notify();
        }
        most > 0.0
    }

    /// Where the tree stands sideways in this frame. It is moved when the open folder's row or the keyboard's row becomes another
    /// one (or the sidebar another width), by the least that shows that row; never for a row that only came into view by scrolling.
    fn sideways(&mut self, mark: Option<(usize, bool)>, focused: bool, window: &mut Window) -> f32 {
        let width = self.tree_w.get();
        if width <= 1.0 {
            return 0.0; // not measured yet: the rows it would be moved for are looked at again when it is
        }
        let rows = self.model.rows();
        let at_row = |i: usize| rows.get(i).map(|r| (r.id, r.depth));
        let marked = mark.and_then(|(i, _)| at_row(i));
        let cursor = if focused { self.model.cursor_row().and_then(at_row) } else { None };
        if rows.len() != self.deepest.0 {
            self.deepest = (rows.len(), rows.iter().map(|r| r.depth).max().unwrap_or(0));
        }
        let mut moved_for = None;
        if (width - self.slid_width).abs() > 0.5 {
            self.slid_width = width;
            moved_for = cursor.or(marked);
        }
        if marked != self.slid_marked {
            self.slid_marked = marked;
            moved_for = marked.or(moved_for);
        }
        if cursor != self.slid_cursor {
            self.slid_cursor = cursor;
            moved_for = cursor.or(moved_for);
        }
        let (now, animated) = (Instant::now(), crate::settings::animations());
        let most = sideways_most(self.deepest.1, width);
        let to = match moved_for {
            Some((_, depth)) => sideways_goal(self.slide.to, indent_of(depth), width).min(most),
            None => self.slide.to.min(most),
        };
        self.slide.go(to, now, animated);
        let (x, sliding) = self.slide.at(now);
        if sliding {
            window.request_animation_frame();
        }
        x
    }

    // ---- drawing -----------------------------------------------------------------------------

    /// Where a menu for a row opens when it comes from the keyboard: under the row, kept inside the sidebar's visible part.
    fn row_anchor(&self, row: usize) -> Point<Pixels> {
        let b = self.scroll.bounds();
        let top = f32::from(b.origin.y) + f32::from(self.scroll.offset().y) + self.tree_top.get() + row as f32 * ROW_H;
        let y = (top + ROW_H).clamp(f32::from(b.origin.y) + ROW_H, f32::from(b.origin.y + b.size.height).max(f32::from(b.origin.y) + ROW_H));
        point(b.origin.x + px(40.), px(y))
    }

    /// Moves the sidebar's scroll position to a wanted row. The content height is known by the time this runs (the rows are
    /// in the model and the tree's box is sized from them), so one try is enough; a second covers the first frame.
    fn apply_goal(&mut self, cx: &mut Context<Self>) {
        let Some(goal) = self.goal else { return };
        let view_h = f32::from(self.scroll.bounds().size.height);
        if view_h <= 1. {
            return; // the scroll area has not been measured yet: keep the goal
        }
        let cur = -f32::from(self.scroll.offset().y);
        let row_top = self.tree_top.get() + goal.row as f32 * ROW_H;
        let want = scroll_goal(cur, view_h, row_top, ROW_H, goal.centre);
        if (want - cur).abs() > 0.5 {
            self.scroll.set_offset(point(self.scroll.offset().x, px(-want)));
        }
        self.goal = (goal.tries > 1).then_some(Goal { tries: goal.tries - 1, ..goal });
        if self.goal.is_some() {
            cx.notify();
        }
    }

    fn row_icon(root: Option<RootKind>, expanded: bool) -> L {
        match root {
            Some(RootKind::Profile) => L::House,
            Some(RootKind::Removable) => L::Usb,
            Some(RootKind::Network) => L::Network,
            Some(RootKind::Optical) => L::Disc3,
            Some(RootKind::Fixed) => L::HardDrive,
            None if expanded => L::FolderOpen,
            None => L::Folder,
        }
    }

    fn render_row(&self, ix: usize, row: Row, frame: &Frame, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Frame { mark, focused, now, aside } = *frame;
        let mut info = self.model.info(&row, now);
        // (one selected row in the whole sidebar: see `shown_elsewhere`)
        info.selected &= !self.shown_elsewhere;
        let id = row.id;
        // Moved sideways, a row keeps its box (its fill, its accent pill and its glass lens stay where they are): what is IN it starts
        // further left, and past the row's own left edge it is cut off (`lead` below zero).
        let lead = 8. + indent_of(row.depth) - aside;
        let pad = lead.max(0.);
        let cut = lead.min(0.);
        let deep = row.depth > TOOLTIP_PAST_LEVEL;
        if row.kind == RowKind::Error {
            let text = if info.not_found { t!("Klasör bulunamadı").to_string() } else { format!("{}: {}", t!("Açılamadı"), info.error.unwrap_or_default()) };
            return h_flex()
                .id(("tree-err", id as usize))
                .h(px(ROW_H))
                .w_full()
                .pl(px((lead + 16. + 4.).max(0.)))
                .pr_2()
                .gap_1()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .text_xs()
                .text_color(p.text_tertiary)
                .hover(|s| s.bg(p.hover))
                .child(Icon::new(L::TriangleAlert).size(px(12.)).flex_shrink_0())
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(text))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.retry(id, cx)))
                .into_any_element();
        }
        let selected = info.selected;
        let lens = self.lens.filter(|_| selected);
        // the open folder's branch is closed (or it is not listed): the closest open folder above it carries a quieter mark
        let covers = !selected && mark == Some((ix, false));
        let cursor = info.cursor && focused;
        let (expanded, can_expand, root, hidden, slow) = (info.expanded, info.can_expand, info.root, info.hidden, info.slow);
        // a customised folder (not a root) shows the icon Windows gives it, as the file list does (issue 65)
        let own_icon = (info.own_icon && root.is_none())
            .then(|| {
                let (dir, name) = (info.path.parent()?.to_path_buf(), info.path.file_name()?.to_string_lossy().into_owned());
                let e = rf_core::Entry::new(&name, rf_core::EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
                self.images.update(cx, |c, cx| c.icon(&dir, &e, cx))
            })
            .flatten();
        let tag_dot = info.path.parent().and_then(|_| crate::tags::first_color(info.path, cx));
        let icon_color = match root {
            Some(RootKind::Profile) => p.accent,
            Some(_) => p.text_secondary,
            None => theme::kind_color(crate::icons::Category::Folder, p.dark),
        };
        let icon_color = if hidden { icon_color.opacity(0.55) } else { icon_color };
        let chevron: AnyElement = if slow {
            // the first listing is taking long: a quiet pulse where the chevron goes
            div()
                .child(Icon::new(L::LoaderCircle).size(px(12.)).text_color(p.text_tertiary))
                .with_animation(("tree-wait", id as usize), Animation::new(Duration::from_millis(WAIT_PULSE_MS)), |el, t| {
                    let t = (t * 3.).fract();
                    el.opacity(0.35 + 0.65 * (1. - (2. * t - 1.).abs()))
                })
                .into_any_element()
        } else if can_expand {
            Icon::new(if expanded { L::ChevronDown } else { L::ChevronRight }).size(px(12.)).text_color(p.text_tertiary).into_any_element()
        } else {
            div().into_any_element()
        };
        let drop_dir = info.path.to_path_buf();
        let (drop_bg, drop_border) = (p.selection, p.accent);
        let full_name = deep.then(|| SharedString::from(crate::rtl::display_text(&info.label)));
        h_flex()
            .id(("tree-row", id as usize))
            .h(px(ROW_H))
            .w_full()
            .pl(px(pad))
            .pr_2()
            .gap_1()
            .items_center()
            .rounded(theme::RADIUS_SM)
            .text_sm()
            .relative()
            .when(cut < 0., |d| d.overflow_hidden())
            .when(selected && lens.is_none(), |d| d.bg(p.selection).child(theme::selection_pill(p)))
            .when_some(lens, |d, (key, hwnd)| {
                // (while the row moves with a scroll the scene paints the pill itself: the engine's lens would trail the row)
                let moving = crate::glass::lens_phase(hwnd, crate::glass::SLOT_SIDEBAR) != crate::glass::LensPhase::Rest;
                d.when(moving, |d| d.child(crate::glass::lens_stand_in(p.dark, ROW_H))).child(crate::glass::lens_probe(crate::glass::SLOT_SIDEBAR, key))
            })
            .when(covers, |d| d.bg(p.hover))
            .when(cursor && !selected, |d| d.bg(p.pressed))
            .when(!selected && !covers && !cursor, |d| d.hover(|s| s.bg(p.hover)))
            .when_some(full_name, |d, name| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(w, cx)))
            .child(
                div()
                    .id(("tree-chev", id as usize))
                    .w(px(16.))
                    .h(px(ROW_H))
                    .flex_shrink_0()
                    .when(cut < 0., |d| d.ml(px(cut)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SM)
                    .when(can_expand, |d| d.hover(|s| s.bg(p.pressed)))
                    .child(chevron)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        // opening a branch is not navigating
                        cx.stop_propagation();
                        window.focus(&this.focus, cx);
                        this.toggle(id, cx);
                    })),
            )
            .child(
                div()
                    .relative()
                    .size(px(16.))
                    .flex_shrink_0()
                    .child(match own_icon {
                        Some(picture) => img(picture).size(px(16.)).into_any_element(),
                        None => Icon::new(Self::row_icon(root, expanded)).size(px(16.)).text_color(icon_color).into_any_element(),
                    })
                    .when_some(tag_dot, |d, c| d.child(crate::tags::corner_dot(c, p.card_bg))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .when(hidden, |d| d.opacity(0.7))
                    .child(crate::rtl::display_text(&info.label)),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.click_row(id, ev.modifiers().control, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.focus, cx);
                    this.model.set_cursor(id);
                    if let Some(path) = this.model.path_of(id).map(Path::to_path_buf) {
                        cx.emit(TreeEvent::Menu(path));
                    }
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    if let Some(path) = this.model.path_of(id).map(Path::to_path_buf) {
                        cx.emit(TreeEvent::OpenTab(path));
                    }
                }),
            )
            // files and folders dropped here go into this folder, exactly as on any other sidebar row
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg).border_1().border_color(drop_border))
            .on_drag_move::<ExternalPaths>(crate::folder_view::drop_effect_over(drop_dir.clone()))
            .on_drop(move |paths: &ExternalPaths, _, cx| drop_files(paths, drop_dir.clone(), cx))
            .into_any_element()
    }
}

impl Render for SidebarTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let now = self.now();
        if let Some(row) = self.model.take_scroll_target() {
            // The sidebar is never scrolled under the pointer: a click on a pinned folder, a drive or a tag is a navigation too, and
            // following it by scrolling took the row the user had just clicked away from under the pointer (the user's report). The
            // branch is still opened and the folder marked; it is scrolled to when the navigation came from somewhere else.
            let sidebar = self.scroll.bounds();
            if !sidebar.contains(&window.mouse_position()) {
                self.goal = Some(Goal { row, centre: true, tries: 2 });
            }
        }
        self.apply_goal(cx);
        let n = self.model.rows().len();
        // Only the rows on the screen are built. The tree is a box as tall as all its rows, inside the sidebar's scroll area;
        // where the scroll area is now tells which rows are in view.
        let view_h = f32::from(self.scroll.bounds().size.height);
        let into_tree = -f32::from(self.scroll.offset().y) - self.tree_top.get();
        let (first, last) = visible_range(n, into_tree, view_h, ROW_H, OVERSCAN, FIRST_FRAME_ROWS);
        let in_view = self.model.marked_row().is_none_or(|(row, _)| (first..last).contains(&row));
        if in_view != self.selected_in_view {
            // (the window decides who has the lens when it builds the sidebar: it is asked again)
            self.selected_in_view = in_view;
            cx.notify();
        }
        let (mark, focused) = (self.model.marked_row().filter(|_| !self.shown_elsewhere), self.focus.is_focused(window));
        let aside = self.sideways(mark, focused, window);
        let frame = Frame { mark, focused, now, aside };
        let rows: Vec<AnyElement> = (first..last)
            .map(|ix| {
                let row = self.model.rows()[ix];
                self.render_row(ix, row, &frame, &p, cx)
            })
            .collect();
        let me = cx.entity();
        let scroll = self.scroll.clone();
        let top = self.tree_top.clone();
        let wide = self.tree_w.clone();
        let probe = canvas(
            move |b, window, cx| {
                let y = f32::from(b.origin.y - scroll.bounds().origin.y - scroll.offset().y);
                let w = f32::from(b.size.width);
                if (top.get() - y).abs() > 0.5 || (wide.get() - w).abs() > 0.5 {
                    top.set(y);
                    wide.set(w);
                    // the rows were chosen with the old place: draw again
                    me.update(cx, |_, cx| cx.notify());
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(1.));
        div()
            .id("folder-tree")
            .relative()
            .w_full()
            .h(px(n as f32 * ROW_H))
            .track_focus(&self.focus)
            .key_context(crate::actions::TREE_CTX)
            // (Tab: the keyboard goes to the file list. As an action, see `actions::TREE_CTX`.)
            .on_action(cx.listener(|_, _: &crate::actions::LeaveTree, _, cx| {
                cx.stop_propagation();
                cx.emit(TreeEvent::Leave);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            // (a sideways turn only: the turn up and down is the sidebar's)
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                let dx = match ev.delta {
                    ScrollDelta::Pixels(d) => f32::from(d.x),
                    // (a line of the wheel is one level: a notch, three lines here, moved the tree 144 px at 48 a line, most of its way)
                    ScrollDelta::Lines(l) => l.x * INDENT,
                };
                // (taken here, or the sidebar would scroll up or down by it as well: seen in a test copy)
                if dx.abs() > 0.01 && this.slide_by(-dx, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(probe)
            .child(v_flex().absolute().top(px(first as f32 * ROW_H)).left_0().w_full().children(rows))
    }
}

// ---- the window's side ---------------------------------------------------------------------------

impl Workspace {
    /// Makes the tree when the setting is on (and drops it when it goes off). Called once per frame before the sidebar is built.
    pub(super) fn ensure_side_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = cx.global::<crate::settings::Settings>().sidebar_tree;
        if !on {
            if self.side_tree.view.is_some() {
                self.side_tree.view = None;
                self.side_tree.sub = None;
            }
            return;
        }
        if self.side_tree.view.is_some() {
            return;
        }
        let (places, scroll) = (self.places.clone(), self.side_tree.scroll.clone());
        let images = self.images.clone();
        let view = cx.new(|cx| SidebarTree::new(places, images, scroll, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &TreeEvent, window, cx| this.on_tree_event(ev, window, cx));
        self.side_tree.view = Some(view);
        self.side_tree.sub = Some(sub);
    }

    /// Whether the sidebar that is being built shows the tree (the setting is on; not the rail, not the rail's peek). The drives are
    /// the tree's roots then, and the separate "Sürücüler" section would show them a second time.
    pub(super) fn shows_tree(&self, rail: bool) -> bool {
        !rail && self.side_tree.view.is_some() && !self.side_tree.in_peek
    }

    /// The folder tree's focus, when the tree is on screen (the setting is on, the sidebar is open and not the rail): a stop of Tab.
    pub(super) fn tree_focus(&self, cx: &App) -> Option<FocusHandle> {
        (self.sidebar_open && self.shows_tree(self.sidebar_rail)).then(|| self.side_tree.view.as_ref().map(|v| v.read(cx).focus.clone())).flatten()
    }

    /// The keyboard came to the tree with Tab: see `SidebarTree::keyboard_entered`.
    pub(super) fn tree_keyboard_entered(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.side_tree.view.clone() {
            t.update(cx, |t, cx| t.keyboard_entered(cx));
        }
    }

    pub(super) fn set_tree_peek(&mut self, peek: bool) {
        self.side_tree.in_peek = peek;
    }

    fn on_tree_event(&mut self, ev: &TreeEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            // not `navigate`: that gives the focus to the file list, and the tree keeps it like Explorer's navigation pane does
            TreeEvent::Navigate(path) => {
                self.active_tab().clone().update(cx, |v, cx| v.navigate(Location::Path(path.clone()), cx));
                self.search.update(cx, |s, cx| s.set_value("", window, cx));
            }
            TreeEvent::OpenTab(path) => {
                let keep = self.active;
                self.open_tab(Location::Path(path.clone()), window, cx);
                self.activate(keep, window, cx);
            }
            TreeEvent::Menu(path) => self.sidebar_menu(path.clone(), window, cx),
            // from the keyboard: not the physical Shift key's classic menu, and next to the row, not at the mouse
            TreeEvent::KeyMenu(path, at) => self.sidebar_menu_at(path.clone(), false, *at, window, cx),
            TreeEvent::Leave => {
                self.focus_active(window, cx);
                self.active_tab().clone().update(cx, |v, cx| v.keyboard_entered(cx));
            }
        }
    }

    /// Adds the "Klasörler" section to the sidebar column (when the setting is on, the sidebar is not the rail) and the sidebar's own
    /// menu on a right click in it (the sidebar's rows that have a menu of their own keep it).
    pub(super) fn append_tree(&mut self, col: Stateful<Div>, cur: &Location, shown_elsewhere: bool, rail: bool, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        if rail {
            return col;
        }
        // With the setting off the sidebar is exactly as it was: not even a handler is added.
        let Some(view) = self.side_tree.view.clone().filter(|_| !self.side_tree.in_peek) else { return col };
        let col = col.on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, _: &MouseDownEvent, window, cx| this.sidebar_own_menu(window, cx)),
        );
        let path = match cur {
            Location::Path(p) => Some(p.clone()),
            _ => None,
        };
        // Over glass the selected row of the sidebar is a lens. No other row has it this frame (they were built first or are known
        // not to be selected): the tree's selected row takes it, and the window is told that the lens is in use.
        let lens = (self.sidebar_lens && !shown_elsewhere && !self.sidebar_lens_claimed.get() && view.read(cx).has_selected_row()).then(|| {
            self.sidebar_lens_claimed.set(true);
            (crate::glass::key_of(cur), self.glass_hwnd)
        });
        view.update(cx, |t, cx| t.sync(path.as_deref(), shown_elsewhere, lens, cx));
        col.track_scroll(&self.side_tree.scroll).child(
            v_flex()
                .child(div().px_2().pt_3().pb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(p.text_tertiary).child(t!("Klasörler")))
                .child(view),
        )
    }

    /// Right click on the sidebar's own ground (only while the tree is on): the tree's two switches.
    fn sidebar_own_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::fluent_menu::{Item, Menu, MenuIcon};
        let s = cx.global::<crate::settings::Settings>();
        let (on, follow) = (s.sidebar_tree, s.sidebar_tree_follow);
        let mut items = vec![Item::action(t!("Klasör ağacı"), MenuIcon::Lucide(L::FolderTree), move |_, cx| {
            crate::settings::update(cx, |s| s.sidebar_tree = !on)
        })
        .checked(on)];
        if on {
            items.push(Item::action(t!("Açık klasörü izle"), MenuIcon::Lucide(L::Crosshair), move |_, cx| {
                crate::settings::update(cx, |s| s.sidebar_tree_follow = !follow)
            })
            .checked(follow));
        }
        crate::fluent_menu::show(Menu { quick: Vec::new(), items }, window.mouse_position(), window, cx);
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the window module's glob imports bring GPUI's own `test` attribute along)
    use super::{DriveKind, Duration, ListFailure, Msg, NavTree, PathBuf, RootKind, RootSpec, list_folders, read_changes, root_kind};
    use std::time::Instant;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-tree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A folder's children stand to the right of it at every depth a tree is opened to, by the same step.
    #[test]
    fn every_level_is_indented_more_than_its_parent() {
        use super::indent_of;
        assert_eq!((indent_of(0), indent_of(1), indent_of(6), indent_of(7), indent_of(40)), (0., 16., 96., 112., 640.));
        for depth in 0..60 {
            assert_eq!(indent_of(depth + 1) - indent_of(depth), super::INDENT, "level {depth} and its children");
        }
    }

    /// A deep row moves the tree sideways by the least that shows it, and a shallow one brings it back by the least.
    #[test]
    fn a_deep_row_moves_the_tree_sideways_by_the_least_that_shows_it() {
        use super::{NAME_ROOM, ROW_LEAD, ROW_TAIL, indent_of, sideways_goal, sideways_most};
        let w = 240.0;
        let need = |depth: u16| indent_of(depth) + ROW_LEAD + NAME_ROOM + ROW_TAIL - w;
        assert_eq!(sideways_goal(0., indent_of(3), w), 0., "a shallow row: nothing moves");
        assert_eq!((need(8), sideways_goal(0., indent_of(8), w)), (40., 40.), "by exactly what its name is short of");
        assert_eq!(sideways_goal(60., indent_of(8), w), 60., "moved enough already: it stays");
        assert_eq!(sideways_goal(100., indent_of(2), w), 32., "moved past a shallow row's start: back to it, no further");
        assert_eq!(sideways_goal(100., indent_of(0), w), 0.);
        assert_eq!(sideways_goal(0., indent_of(1), 60.), 16., "a sidebar too narrow for the name's room: the row's start wins");
        assert_eq!(sideways_goal(12., indent_of(9), 0.), 12., "not measured yet: left as it is");
        // by hand: as far as the deepest row needs, and not at all when no row is deep
        assert_eq!((sideways_most(8, w), sideways_most(2, w), sideways_most(30, 0.)), (40., 0., 0.));
        // and whatever the depth, the row it was moved for has its start in view and its name its room
        for depth in 0..40u16 {
            let x = sideways_goal(0., indent_of(depth), w);
            assert!(x <= indent_of(depth), "level {depth}: the chevron is in view");
            assert!(indent_of(depth) + ROW_LEAD + NAME_ROOM + ROW_TAIL - x <= w + 0.01, "level {depth}: the name has its room");
        }
    }

    /// The slide starts where the tree is, ends where it was sent, and turned around in mid flight it goes on from where it is.
    #[test]
    fn the_slide_ends_where_it_was_sent() {
        use super::{SLIDE_MS, Slide};
        let t0 = std::time::Instant::now();
        let after = |ms: f32| t0 + std::time::Duration::from_secs_f32(ms / 1000.0);
        let mut s = Slide::REST;
        s.go(80., t0, true);
        assert_eq!(s.at(t0), (0., true));
        let (mid, moving) = s.at(after(SLIDE_MS / 2.0));
        assert!(moving && mid > 0. && mid < 80., "{mid}");
        assert_eq!(s.at(after(SLIDE_MS)), (80., false));
        // turned around half way: from where it is, not from an end
        s.go(0., after(SLIDE_MS / 2.0), true);
        assert_eq!(s.at(after(SLIDE_MS / 2.0)).0, mid);
        assert_eq!(s.at(after(SLIDE_MS * 2.0)), (0., false));
        // with animations off it is there at once; a target half a pixel away is the same place
        let mut s = Slide::REST;
        s.go(50., t0, false);
        assert_eq!(s.at(t0), (50., false));
        s.go(50.3, t0, true);
        assert_eq!(s, Slide { from: 0., to: 50., since: None });
    }

    #[test]
    fn a_listing_has_the_folders_only_with_their_flags() {
        let d = scratch("list");
        for n in ["b", "a10", "a2", "gizli"] {
            std::fs::create_dir(d.join(n)).unwrap();
        }
        std::fs::write(d.join("dosya.txt"), b"x").unwrap();
        // a hidden folder: the attribute is set on the folder this test made
        let ok = std::process::Command::new("attrib").arg("+h").arg(d.join("gizli")).status().map(|s| s.success()).unwrap_or(false);
        let mut got = list_folders(&d).unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = got.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a10", "a2", "b", "gizli"], "a file is not a tree node");
        if ok {
            assert!(got.iter().find(|i| i.name == "gizli").unwrap().hidden);
        }
        assert!(!got.iter().find(|i| i.name == "b").unwrap().hidden);
        assert_eq!(list_folders(&d.join("yok")), Err(ListFailure::NotFound));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_watcher_names_become_folders_or_nothing() {
        let d = scratch("changes");
        std::fs::create_dir(d.join("Yeni")).unwrap();
        std::fs::write(d.join("dosya.txt"), b"x").unwrap();
        let got = read_changes(&d, vec!["yeni".into(), "dosya.txt".into(), "yok".into()]).unwrap();
        assert_eq!(got[0].item.as_ref().map(|i| i.name.as_str()), Some("Yeni"), "the name as the disk spells it");
        assert!(got[1].item.is_none() && got[2].item.is_none());
        // a name that can't be a file name can't be told
        assert!(read_changes(&d, vec!["a*b".into()]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The whole chain on a real disk: an open node is listed, watched, and follows a folder made, renamed and removed in it.
    #[test]
    fn an_open_node_follows_the_disk() {
        let d = scratch("follow");
        std::fs::create_dir(d.join("birinci")).unwrap();
        let mut tree = NavTree::new();
        tree.set_roots(vec![RootSpec { path: d.clone(), label: "kök".into(), kind: RootKind::Fixed }], 0);
        let root = tree.rows()[0].id;
        let req = tree.toggle(root, 0).remove(0);
        tree.apply_listing(&req, list_folders(&req.path), 1);
        let names = |t: &NavTree| t.rows().iter().skip(1).map(|r| t.info(r, 0).label.to_string()).collect::<Vec<_>>();
        assert_eq!(names(&tree), ["birinci"]);
        // watch what the tree says to watch, and feed what the watcher reports back into the model, as the view does
        assert_eq!(tree.watch_set(), std::slice::from_ref(&d));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let dir = d.clone();
        let w = rf_win::watch::watch_dir_changes(&d, Duration::from_millis(100), move |ch| {
            let msg = if ch.rescan || ch.names.is_empty() {
                Msg::Rescan(dir.clone())
            } else {
                match read_changes(&dir, ch.names) {
                    Ok(c) => Msg::Changes(dir.clone(), c),
                    Err(_) => Msg::Rescan(dir.clone()),
                }
            };
            let _ = tx.send(msg);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let settle = |tree: &mut NavTree, want: &[&str]| {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if let Ok(m) = rx.recv_timeout(Duration::from_millis(100)) {
                    match m {
                        Msg::Changes(dir, c) => {
                            tree.apply_changes(&dir, c);
                        }
                        Msg::Rescan(dir) => {
                            for r in tree.rescan(&dir) {
                                tree.apply_listing(&r, list_folders(&r.path), 2);
                            }
                        }
                    }
                }
                if names(tree) == want {
                    return;
                }
            }
            panic!("the tree shows {:?}, wanted {want:?}", names(tree));
        };
        std::fs::create_dir(d.join("ikinci")).unwrap();
        settle(&mut tree, &["birinci", "ikinci"]);
        std::fs::rename(d.join("birinci"), d.join("yeni ad")).unwrap();
        settle(&mut tree, &["ikinci", "yeni ad"]);
        std::fs::remove_dir(d.join("ikinci")).unwrap();
        settle(&mut tree, &["yeni ad"]);
        // a file made in it changes nothing the tree shows
        std::fs::write(d.join("dosya.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        while let Ok(Msg::Changes(dir, c)) = rx.try_recv() {
            assert!(!tree.apply_changes(&dir, c));
        }
        assert_eq!(names(&tree), ["yeni ad"]);
        drop(w);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn drive_kinds_map_to_root_kinds() {
        assert_eq!(root_kind(DriveKind::Removable), RootKind::Removable);
        assert_eq!(root_kind(DriveKind::Network), RootKind::Network);
        assert_eq!(root_kind(DriveKind::Optical), RootKind::Optical);
        assert_eq!(root_kind(DriveKind::Fixed), RootKind::Fixed);
        assert_eq!(root_kind(DriveKind::RamDisk), RootKind::Fixed);
    }
}
