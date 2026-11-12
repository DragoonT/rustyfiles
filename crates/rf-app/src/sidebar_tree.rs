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
/// Pixels of indent per level.
const INDENT: f32 = 16.;
/// Levels that indent: deeper rows stand at this indent, so that the name keeps room in a narrow sidebar (the row's tooltip has the
/// whole name).
const MAX_INDENT_LEVELS: u16 = 6;
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
    /// The sidebar's scroll area: the tree is part of its content and scrolls with it.
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// Where the top of the tree is in the scroll area's content, in pixels (measured at every paint).
    tree_top: Rc<Cell<f32>>,
    epoch: Instant,
    goal: Option<Goal>,
    last_roots: Vec<(PathBuf, String, RootKind)>,
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
    DirItem { name: e.name.to_string(), label: e.label.as_deref().map(str::to_string), hidden: e.is_hidden(), system: e.is_system() }
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
    fn new(places: Entity<Places>, scroll: ScrollHandle, cx: &mut Context<Self>) -> Self {
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
        let mut this = Self {
            model: NavTree::new(),
            places,
            scroll,
            focus: cx.focus_handle(),
            tree_top: Rc::new(Cell::new(0.)),
            epoch: Instant::now(),
            goal: None,
            last_roots: Vec::new(),
            home: dirs::home_dir(),
            followed: None,
            watchers: HashMap::new(),
            pending_watch: HashSet::new(),
            wanted: HashSet::new(),
            tx,
            _rx_task: task,
            _subs: vec![sub],
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
    pub fn sync(&mut self, cur: Option<&Path>, cx: &mut Context<Self>) {
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
        let Frame { mark, focused, now } = *frame;
        let info = self.model.info(&row, now);
        let id = row.id;
        let pad = 8. + INDENT * f32::from(row.depth.min(MAX_INDENT_LEVELS));
        let deep = row.depth > MAX_INDENT_LEVELS;
        if row.kind == RowKind::Error {
            let text = if info.not_found { t!("Klasör bulunamadı").to_string() } else { format!("{}: {}", t!("Açılamadı"), info.error.unwrap_or_default()) };
            return h_flex()
                .id(("tree-err", id as usize))
                .h(px(ROW_H))
                .w_full()
                .pl(px(pad + 16. + 4.))
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
        // the open folder's branch is closed (or it is not listed): the closest open folder above it carries a quieter mark
        let covers = !selected && mark == Some((ix, false));
        let cursor = info.cursor && focused;
        let (expanded, can_expand, root, hidden, slow) = (info.expanded, info.can_expand, info.root, info.hidden, info.slow);
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
        let full_name = deep.then(|| SharedString::from(info.label.to_string()));
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
            .when(selected, |d| d.bg(p.selection).child(theme::selection_pill(p)))
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
            .child(Icon::new(Self::row_icon(root, expanded)).size(px(16.)).text_color(icon_color).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .when(hidden, |d| d.opacity(0.7))
                    .child(SharedString::from(info.label.to_string())),
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
        let frame = Frame { mark: self.model.marked_row(), focused: self.focus.is_focused(window), now };
        let rows: Vec<AnyElement> = (first..last)
            .map(|ix| {
                let row = self.model.rows()[ix];
                self.render_row(ix, row, &frame, &p, cx)
            })
            .collect();
        let me = cx.entity();
        let scroll = self.scroll.clone();
        let top = self.tree_top.clone();
        let probe = canvas(
            move |b, window, cx| {
                let y = f32::from(b.origin.y - scroll.bounds().origin.y - scroll.offset().y);
                if (top.get() - y).abs() > 0.5 {
                    top.set(y);
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
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
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
        let view = cx.new(|cx| SidebarTree::new(places, scroll, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &TreeEvent, window, cx| this.on_tree_event(ev, window, cx));
        self.side_tree.view = Some(view);
        self.side_tree.sub = Some(sub);
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
            TreeEvent::Leave => self.focus_active(window, cx),
        }
    }

    /// Adds the "Klasörler" section to the sidebar column (when the setting is on, the sidebar is not the rail) and the sidebar's own
    /// menu on a right click in it (the sidebar's rows that have a menu of their own keep it).
    pub(super) fn append_tree(&mut self, col: Stateful<Div>, cur: &Location, rail: bool, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
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
        view.update(cx, |t, cx| t.sync(path.as_deref(), cx));
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
