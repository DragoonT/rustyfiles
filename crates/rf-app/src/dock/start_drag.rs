//! Drag and drop in the Start panel, drawn and driven. A child module of `start.rs`: it works on that view's own state. The rules (click or
//! drag, where a point is, what may be dropped where, the order of the pins, the slide) are `rf_win::dock::startdrag`, pure and tested;
//! this file is the part that reads the mouse and puts the result on the screen.
//!
//! * A press on a pinned tile, an app of "Tümü" or a recommended entry is remembered (`press`). If the pointer moves more than the
//!   system's drag distance the press becomes a drag; if it does not, nothing was done and the click starts the app as before.
//! * THE MOUSE. GPUI holds the mouse for the window from the press on, so the moves and the release keep coming while the pointer is
//!   outside the panel; they are heard with window-level listeners (`listeners`, the desktop's way), not element ones.
//! * A PINNED TILE follows the pointer; the others slide to make room (`Slides`, the dock's own ease); letting go over the grid puts
//!   it there and writes the pin file at once (`commit_reorder`). The panel stays open.
//! * OVER THE DOCK'S MIDDLE ISLAND (screen position against the layout's glass rectangle) the island is highlighted (`drop_hint`) and
//!   letting go adds the app to the end of the dock's own pins (`own_pins::pin_start_app`); the panel closes.
//! * OUT IN THE OPEN: an app that has a shortcut in the Start menu's folders becomes a real file drag of a COPY of that shortcut, the
//!   system's own drag (`rf_win::dnd`, the one the file list uses), copy or link only, the moment the pointer leaves the panel; a
//!   packaged app has no such file and the pointer says "not here". The shortcut is only LOOKED FOR when the press becomes a drag (no
//!   write); the copy is made when the pointer first leaves the panel, so a reorder inside it writes nothing to the disk. A recommended
//!   file is a real file drag from the first moment the press has moved.
//! * THE DOCK AFTER THE HAND-OVER. The way to the island runs over the desktop, so a classic app's drag has become the system's by
//!   the time the pointer is over the island. The island is therefore watched while the system's loop runs (lit while the pointer is
//!   over it) and a drop that ends over it adds the app to the dock as well (`hand_to_system`), but only when the window under the
//!   pointer is the island's own: what took the file is then the island, not another program behind its rectangle. A packaged app
//!   never leaves the panel's own drag and is dropped on the island directly.
//! * Escape, the right mouse button, the pointer being lost (the left button is no longer down and no release came) and the panel
//!   closing put everything back. After a drag the release is not a click (`ClickGate`).
//!
//! WHAT IT COSTS. At rest: three window-level listeners that return at once, and one empty canvas that remembers the grid's bounds.
//! During a drag: a redraw per mouse move, a 50 ms look at the left button, and (apps) one directory walk of the Start menu's folders
//! on a worker thread (cached); the copy of one small file is made on a worker only when the pointer leaves the panel.

use std::cell::Cell;
use std::path::PathBuf;

use rf_win::dock::startdrag::{self as rules, Feedback, Grid, Item, Scene, Slides, Where};

use super::*;
use crate::dock::host::{self, Kind};

/// The folder in the system's temp folder that holds the copies of shortcuts a drag carries.
const COPY_FOLDER: &str = "rustyfiles-start-drag";
/// A copy is kept this long (a target may still be reading it a while after the drop).
const COPIES_KEPT: Duration = Duration::from_secs(3600);
/// How often a drag looks at the left button.
const POLL: Duration = Duration::from_millis(50);
/// The ghost of an app taken from the list.
const GHOST_ROW: (f32, f32) = (220.0, 40.0);

/// What was picked up.
#[derive(Clone)]
pub(super) enum Picked {
    /// The pinned tile at this slot (an index among the pins that are shown).
    Pin { app: AppEntry, slot: usize },
    /// A row of the all-apps list.
    Listed { app: AppEntry },
    /// A recommended file.
    Rec { path: PathBuf },
}

impl Picked {
    fn item(&self) -> Item {
        match self {
            Picked::Pin { .. } => Item::Pinned,
            Picked::Listed { .. } => Item::Listed,
            Picked::Rec { .. } => Item::Recommended,
        }
    }

    fn app(&self) -> Option<&AppEntry> {
        match self {
            Picked::Pin { app, .. } | Picked::Listed { app } => Some(app),
            Picked::Rec { .. } => None,
        }
    }

    fn slot(&self) -> Option<usize> {
        match self {
            Picked::Pin { slot, .. } => Some(*slot),
            _ => None,
        }
    }
}

/// A button went down on something that can be picked up; it is a click until the pointer has moved far enough.
struct Press {
    picked: Picked,
    at: Point<Pixels>,
    /// Where in the tile the press was (the ghost keeps that place under the pointer).
    grab: Point<Pixels>,
}

/// The app's shortcut for a drag out of the panel.
enum Shortcut {
    /// Being looked for (a worker thread, read only).
    Looking,
    /// The Start-menu shortcut is known; nothing has been written yet (a reorder inside the panel never needs a copy).
    Found(PathBuf),
    /// The pointer has left the panel and the copy a drag carries is being made (a worker thread).
    Copying,
    /// A copy of the Start-menu shortcut is ready to be dragged.
    Ready(PathBuf),
    /// The app has no shortcut file (a packaged app, or none by its name): it can go to the dock only.
    Missing,
}

impl Shortcut {
    /// Whether the app may be dragged out of the panel as a file (the pointer says so before the copy is ready).
    fn may_leave(&self) -> bool {
        matches!(self, Shortcut::Found(_) | Shortcut::Copying | Shortcut::Ready(_))
    }
}

/// A drag in progress inside the window.
struct Carry {
    picked: Picked,
    pos: Point<Pixels>,
    grab: Point<Pixels>,
    /// The drop slot the pinned tile is held over, while it is over the grid.
    over: Option<usize>,
    /// The page of the grid that is shown: its first slot and its tile count.
    first: usize,
    count: usize,
    slides: Slides,
    shortcut: Shortcut,
    /// The pointer is out of the panel and the grid, where an app is dropped as a file (as of the last move).
    outside: bool,
    /// The look at the left button that finds a lost pointer, and the last mouse event heard (the look is trusted only after quiet).
    lost: rules::LostPointer,
    last_event: Instant,
}

/// Everything the drag keeps. `Default` is "no drag".
#[derive(Default)]
pub(super) struct DragState {
    press: Option<Press>,
    carry: Option<Carry>,
    /// The system's file drag is running (its own loop owns the mouse).
    handed_over: bool,
    /// A drag ended, so the release that follows is not a click.
    click: rules::ClickGate,
    /// The pinned grid's bounds as of the last frame (panel window coordinates).
    pub(super) grid: Rc<Cell<Option<Bounds<Pixels>>>>,
    poll: Option<Task<()>>,
    shortcut_task: Option<Task<()>>,
}

impl DragState {
    /// The system's loop is over (or never answered): the panel's own drag state is free again and the island is no longer watched.
    fn end_hand_over(&mut self) {
        self.handed_over = false;
        self.poll = None;
    }

    /// Puts a press or a drag in the window back. True when there was one. A drag that is in the SYSTEM's loop is not touched: its
    /// island watch must go on until the loop ends (the loop's end clears it), whatever closed the panel meanwhile.
    fn cancel(&mut self) -> bool {
        if self.handed_over {
            return false;
        }
        let was = self.carry.is_some() || self.press.is_some();
        self.press = None;
        self.carry = None;
        self.poll = None;
        self.shortcut_task = None;
        if was {
            self.click.drag_ended();
        }
        was
    }

    /// A new opening of the panel: nothing from before survives, not even a hand-over whose end never came.
    fn reset_open(&mut self) {
        self.handed_over = false;
        self.cancel();
        self.click = rules::ClickGate::default();
    }
}

/// The primary mouse button is down now (as the user set it: the buttons may be swapped).
fn primary_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    unsafe {
        // (GetAsyncKeyState reads the PHYSICAL buttons: with the buttons swapped the one under the index finger is the right one)
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        (GetAsyncKeyState(i32::from(vk.0)) as u16 & 0x8000) != 0
    }
}

/// The pointer for "this cannot be dropped here". The ordinary pointer needs nothing: the window sets it itself at every move.
fn show_feedback(fb: Feedback) {
    if fb == Feedback::NotAllowed {
        use windows::Win32::UI::WindowsAndMessaging::{IDC_NO, LoadCursorW, SetCursor};
        unsafe {
            if let Ok(c) = LoadCursorW(None, IDC_NO) {
                SetCursor(Some(c));
            }
        }
    }
}

/// A window's outer rectangle, screen device px.
fn window_rect(hwnd: isize) -> Option<rf_win::dock::layout::Rect> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut r = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r).ok()? };
    Some(rf_win::dock::layout::Rect::new(r.left, r.top, r.right, r.bottom))
}

/// The top-level window at a screen point (device px), if any.
fn window_at(at: Option<(i32, i32)>) -> Option<isize> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, WindowFromPoint};
    let (x, y) = at?;
    let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
    if hwnd.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    Some(if root.0.is_null() { hwnd.0 as isize } else { root.0 as isize })
}

/// Whether a screen point is on the dock's middle island: inside its glass and the window there is the island's own.
fn on_island(at: Option<(i32, i32)>, cx: &App) -> bool {
    // (the middle island of any monitor's dock)
    host::dock_monitors(cx).into_iter().any(|(mon, _)| {
        let island = host::island_window_on(mon, Kind::Middle, cx).map(|(_, h)| h);
        rules::dropped_on_island(at, host::visible_glass_rect_on(mon, Kind::Middle, cx)) && rules::is_island_window(window_at(at), island)
    })
}

fn copy_root() -> PathBuf {
    std::env::temp_dir().join(COPY_FOLDER)
}

/// Makes the copy of the shortcut that a drag carries. Writes the disk: a worker thread.
fn prepare_copy(link: &std::path::Path) -> Option<PathBuf> {
    let root = copy_root();
    let gone = rules::sweep(&root, COPIES_KEPT);
    if gone > 0 {
        log::debug!("start drag: {gone} old shortcut copies removed");
    }
    match rules::drag_copy(link, &root) {
        Ok(copy) => Some(copy),
        Err(e) => {
            log::warn!("start drag: cannot copy an app's shortcut for a drag: {e}");
            None
        }
    }
}

impl StartView {
    // ------------------------------------------------------------------ the state

    /// A drag is going on (in the window or in the system's loop).
    pub(super) fn drag_active(&self) -> bool {
        self.drag.carry.is_some() || self.drag.handed_over
    }

    /// The slot of the pinned tile that is held, if one is.
    pub(super) fn held_pin(&self) -> Option<usize> {
        self.drag.carry.as_ref().and_then(|c| c.picked.slot())
    }

    /// Whether a click may do its work: a click that ends a drag does not (the first call after a drag says no, once).
    pub(super) fn click_allowed(&mut self) -> bool {
        self.drag.click.allow()
    }

    /// Where tile `local` of the page is drawn while a tile is held, in cells (it slides).
    pub(super) fn slide_cell(&self, local: usize, now_ms: u64) -> (f32, f32) {
        self.drag.carry.as_ref().map_or((0.0, 0.0), |c| c.slides.pos(local, now_ms))
    }

    /// Whether the tiles are still sliding (another frame is wanted).
    pub(super) fn sliding(&self, now_ms: u64) -> bool {
        self.drag.carry.as_ref().is_some_and(|c| !c.slides.settled(now_ms))
    }

    /// The cell the held tile would land in, in cells: the empty place in the grid.
    pub(super) fn landing_cell(&self) -> Option<(f32, f32)> {
        let c = self.drag.carry.as_ref()?;
        let from = rules::local_slot(c.picked.slot()?, c.first, c.count)?;
        let over = c.over.map(|s| rules::clamped_local_slot(s, c.first, c.count));
        let cell = *rules::cells_while_held(c.count, from, over).get(from)?;
        Some(((cell % page::COLS) as f32, (cell / page::COLS) as f32))
    }

    /// The first slot and the number of tiles of the grid's page that is shown.
    fn page_tiles(&self) -> (usize, usize) {
        let r = page::page_range(self.grid_page, self.layout.per_page, self.pin_cells.len());
        (r.start, r.len())
    }

    /// The top left corner, in the window, of the pinned tile at `slot` (a slot of the page that is shown), once the grid has been drawn.
    pub(super) fn tile_origin(&self, slot: usize) -> Option<Point<Pixels>> {
        let b = self.drag.grid.get()?;
        let (first, count) = self.page_tiles();
        let local = rules::local_slot(slot, first, count)?;
        Some(point(b.origin.x + px(CELL_W * (local % page::COLS) as f32), b.origin.y + px(page::TILE_H * (local / page::COLS) as f32)))
    }

    fn grid_box(&self) -> Option<Grid> {
        let b = self.drag.grid.get()?;
        let (first, count) = self.page_tiles();
        Some(Grid { origin: (f32::from(b.origin.x), f32::from(b.origin.y)), cell: (CELL_W, page::TILE_H), cols: page::COLS, rows: self.layout.rows, first, count })
    }

    /// The rectangles a point is judged by. The grid is there only while the home page shows it.
    fn drag_scene(&self, window: &Window, cx: &App) -> Scene {
        let size = window.viewport_size();
        let grid = if self.page == Page::Home && self.query.trim().is_empty() { self.grid_box() } else { None };
        Scene { viewport: (f32::from(size.width), f32::from(size.height)), grid, island: host::visible_glass_rect(Kind::Middle, cx) }
    }

    // ------------------------------------------------------------------ drawing: the probes

    /// Remembers the pinned grid's bounds for the next mouse move (an empty element the size of the grid).
    pub(super) fn grid_probe(&self) -> AnyElement {
        let seen = self.drag.grid.clone();
        canvas(move |bounds, _, _| seen.set(Some(bounds)), |_, _, _, _| {}).absolute().size_full().into_any_element()
    }

    /// The window-level listeners: the moves, the release and the presses, in the capture phase, so that they are heard wherever the
    /// pointer is while the window holds the mouse. Each returns at once when no press or drag is going on.
    pub(super) fn drag_listeners(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        t.update(cx, |v, cx| v.drag_move(ev, window, cx));
                    }
                });
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        t.update(cx, |v, cx| v.drag_up(ev, window, cx));
                    }
                });
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        t.update(cx, |v, cx| v.drag_down(ev, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full().into_any_element()
    }

    // ------------------------------------------------------------------ the press

    /// The primary button went down on something that can be picked up. `tile_origin` is the tile's top left corner in the window, when
    /// it is known (a pinned tile).
    pub(super) fn drag_press(&mut self, picked: Picked, at: Point<Pixels>, tile_origin: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        // (before the pins have been read a reorder would write over a list that is still to come)
        let ready = self.live() && !self.drag_active() && (!matches!(picked, Picked::Pin { .. }) || self.pins_loaded) && self.app_menu.is_none() && self.power == PowerMenu::Closed;
        if !ready {
            self.drag.press = None;
            return;
        }
        let grab = match tile_origin {
            Some(o) => point(at.x - o.x, at.y - o.y),
            None => point(px(GHOST_ROW.1 / 2.0), px(GHOST_ROW.1 / 2.0)),
        };
        self.drag.press = Some(Press { picked, at, grab });
        cx.notify();
    }

    /// The window-level listener for a press: a new primary press forgets the last drag's silenced click; any other button during a
    /// drag puts everything back (and is not passed on, so a right button does not also open a menu).
    fn drag_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        if ev.button == MouseButton::Left {
            self.drag.click.pressed();
            self.drag.press = None;
            return;
        }
        if self.drag.carry.is_some() {
            self.cancel_drag(cx);
            cx.stop_propagation();
        }
    }

    // ------------------------------------------------------------------ the moves

    fn drag_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown || self.drag.handed_over || (self.drag.press.is_none() && self.drag.carry.is_none()) {
            return;
        }
        let held = ev.pressed_button == Some(MouseButton::Left);
        if self.drag.carry.is_none() {
            let Some(press) = &self.drag.press else { return };
            if !held {
                // (the button is up and no release came: the press is over)
                self.drag.press = None;
                return;
            }
            let scale = window.scale_factor().max(0.5);
            let (dx, dy) = rules::drag_distance();
            let d = ev.position - press.at;
            if !rules::moved_enough(f32::from(d.x), f32::from(d.y), (dx as f32 / scale, dy as f32 / scale)) {
                return;
            }
            self.begin_drag(cx);
        }
        if !held {
            // moves with no button down while a drag is going: the pointer was lost
            self.cancel_drag(cx);
            return;
        }
        self.carry_to(ev.position, window, cx);
    }

    /// The press has moved far enough: it is a drag.
    fn begin_drag(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.drag.press.take() else { return };
        // whatever happens now, the release is not a click
        self.drag.click.drag_ended();
        let Press { picked, at, grab } = press;
        if let Picked::Rec { path } = &picked {
            // a file is a real drag at once: the target decides what a drop means
            self.hand_to_system(vec![path.clone()], None, cx);
            return;
        }
        let (first, count) = self.page_tiles();
        let cells: Vec<(f32, f32)> = (0..count).map(|j| ((j % page::COLS) as f32, (j / page::COLS) as f32)).collect();
        let duration = if crate::settings::animations() { rules::SLIDE_MS } else { 0 };
        let looking = picked.app().cloned();
        let shortcut = if picked.app().is_some() { Shortcut::Looking } else { Shortcut::Missing };
        self.drag.carry = Some(Carry {
            picked,
            pos: at,
            grab,
            over: None,
            first,
            count,
            slides: Slides::at_rest(&cells, duration),
            shortcut,
            outside: false,
            lost: rules::LostPointer::default(),
            last_event: Instant::now(),
        });
        // the app's shortcut is looked for now (a read, cached), so that the pointer can say at once whether the app may leave the panel;
        // the copy that is carried is made only when it does leave (`start_copy`), so a reorder inside the panel writes nothing
        if let Some(app) = looking {
            self.drag.shortcut_task = Some(cx.spawn(async move |this, cx| {
                let who = app.id.clone();
                let link = cx.background_executor().spawn(async move { rules::shortcut_of(&app) }).await;
                let _ = this.update(cx, |me, cx| {
                    if let Some(c) = me.drag.carry.as_mut()
                        && c.picked.app().is_some_and(|a| a.id == who)
                    {
                        c.shortcut = link.map_or(Shortcut::Missing, Shortcut::Found);
                        cx.notify();
                    }
                });
            }));
        }
        // a pointer that was lost without the release being heard (the capture taken by another window) is found by looking at the button
        self.drag.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if !this.update(cx, |me, cx| me.poll_pointer(cx)).unwrap_or(false) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// One look at the left button. False when the drag is over.
    fn poll_pointer(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(c) = self.drag.carry.as_mut() else { return false };
        // (the release itself is heard as a mouse event and ends the drag with its drop; a button that looks up for a few looks in a row
        // while no mouse event comes either is a lost pointer. Events that keep coming settle it: touch and pen do not set the button.)
        let quiet = u64::try_from(c.last_event.elapsed().as_millis()).unwrap_or(u64::MAX);
        if c.lost.look(primary_down(), quiet) {
            self.cancel_drag(cx);
            return false;
        }
        true
    }

    /// The held thing is at `pos` (panel window coordinates): where it is, what that means, what to show.
    fn carry_to(&mut self, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let scene = self.drag_scene(window, cx);
        let screen = geo::cursor().unwrap_or((i32::MIN, i32::MIN));
        let Some(c) = self.drag.carry.as_mut() else { return };
        c.pos = pos;
        c.last_event = Instant::now();
        let at = rules::where_is((f32::from(pos.x), f32::from(pos.y)), screen, &scene);
        let item = c.picked.item();
        let system_ok = c.shortcut.may_leave();
        let fb = rules::feedback(item, at, c.picked.slot(), system_ok);

        // the other tiles make room for a pinned tile held over the grid
        let over = match (item, at) {
            (Item::Pinned, Where::Grid(slot)) => Some(slot),
            _ => None,
        };
        if over != c.over {
            c.over = over;
            if let Some(from) = c.picked.slot().and_then(|s| rules::local_slot(s, c.first, c.count)) {
                let held_over = over.map(|s| rules::clamped_local_slot(s, c.first, c.count));
                let cells: Vec<(f32, f32)> = rules::cells_while_held(c.count, from, held_over).into_iter().map(|cell| ((cell % page::COLS) as f32, (cell / page::COLS) as f32)).collect();
                c.slides.retarget(&cells, crate::dock::backdrop::now_ms());
            }
        }

        // out in the open with a shortcut to carry: the system's drag takes over (once the copy it carries has been made)
        let leave = rules::drop_for(item, at, c.picked.slot(), system_ok) == rules::Drop::HandToSystem;
        c.outside = leave;
        let app = c.picked.app().cloned();
        let mut hand = None;
        let mut copy_of = None;
        if leave {
            match &c.shortcut {
                Shortcut::Ready(p) => hand = Some(p.clone()),
                Shortcut::Found(link) => {
                    copy_of = Some(link.clone());
                    c.shortcut = Shortcut::Copying;
                }
                _ => {}
            }
        }
        crate::dock::drop_hint::set(fb == Feedback::DockTakes && !leave, cx);
        show_feedback(fb);
        if let Some(copy) = hand {
            self.hand_to_system(vec![copy], app, cx);
            return;
        }
        if let (Some(link), Some(app)) = (copy_of, app) {
            self.start_copy(link, app.id, cx);
        }
        cx.notify();
    }

    /// The pointer has left the panel with an app whose shortcut is known: the copy a drag carries is made on a worker thread. When it is
    /// ready and the pointer is still outside, the system's drag takes over at once (the pointer need not move again).
    fn start_copy(&mut self, link: PathBuf, who: String, cx: &mut Context<Self>) {
        self.drag.shortcut_task = Some(cx.spawn(async move |this, cx| {
            let copy = cx.background_executor().spawn(async move { prepare_copy(&link) }).await;
            let _ = this.update(cx, |me, cx| {
                let Some(c) = me.drag.carry.as_mut() else { return };
                let Some(app) = c.picked.app().filter(|a| a.id == who).cloned() else { return };
                if !matches!(c.shortcut, Shortcut::Copying) {
                    return;
                }
                let Some(copy) = copy else {
                    c.shortcut = Shortcut::Missing;
                    cx.notify();
                    return;
                };
                c.shortcut = Shortcut::Ready(copy.clone());
                if c.outside {
                    me.hand_to_system(vec![copy], Some(app), cx);
                } else {
                    cx.notify();
                }
            });
        }));
    }

    // ------------------------------------------------------------------ the release

    fn drag_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left {
            return;
        }
        self.drag.press = None;
        let scene = self.drag_scene(window, cx);
        let screen = geo::cursor().unwrap_or((i32::MIN, i32::MIN));
        let Some(c) = self.drag.carry.take() else { return };
        self.drag.poll = None;
        self.drag.shortcut_task = None;
        crate::dock::drop_hint::set(false, cx);
        // (the release ends a drag: it is not a click on whatever lies under it)
        self.drag.click.drag_ended();
        let at = rules::where_is((f32::from(ev.position.x), f32::from(ev.position.y)), screen, &scene);
        let system_ok = c.shortcut.may_leave();
        match rules::drop_for(c.picked.item(), at, c.picked.slot(), system_ok) {
            rules::Drop::Reorder { to } => {
                if let Some(from) = c.picked.slot() {
                    self.commit_reorder(from, to, cx);
                }
            }
            rules::Drop::PinToDock => {
                if let Some(app) = c.picked.app().cloned() {
                    crate::dock::own_pins::pin_start_app(app, cx);
                    // a drop outside the panel: the panel closes
                    self.finish(cx);
                }
            }
            // (a shortcut that came too late, or none: the pointer said so; everything stays as it was)
            rules::Drop::HandToSystem | rules::Drop::Nothing | rules::Drop::NotAllowed => {}
        }
        cx.notify();
    }

    /// The pinned app at slot `from` goes to slot `to` of the shown pins; the file is written at once. Pins of apps that are not installed
    /// keep their places in the file.
    fn commit_reorder(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if !self.pins_loaded || from == to {
            return;
        }
        // (the same choice of pins as the grid's, `PinFile::resolve`: a slot of the grid is an index into this list)
        let shown = rules::shown_in(&self.pins, &self.apps);
        let perm = rules::apply_move(self.pins.pins.len(), &shown, from, to);
        self.pins.pins = rules::permuted(&self.pins.pins, &perm);
        self.rebuild_pin_cells();
        save_pins(self.pins.clone());
        // (the keyboard cursor stays on the tile it was on, which has moved: the cursor is dropped rather than left on another app)
        self.spot = None;
        cx.notify();
    }

    // ------------------------------------------------------------------ putting everything back

    /// Escape, the right button, a lost pointer, the panel closing: the drag ends and nothing has changed. The release that may still
    /// come is not a click.
    pub(super) fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.cancel() {
            crate::dock::drop_hint::set(false, cx);
            cx.notify();
        }
    }

    /// A new opening of the panel: no drag and no silenced click, and no hand-over to the system whose end was never heard.
    pub(super) fn reset_drag(&mut self, cx: &mut Context<Self>) {
        self.drag.reset_open();
        crate::dock::drop_hint::set(false, cx);
    }

    // ------------------------------------------------------------------ the system's file drag

    /// The system's own file drag takes over (the one the file list uses, `rf_win::dnd`; a copy or a link only, so nothing is MOVED out
    /// of the Start menu or the user's folders by a drop on a program that moves by default). It runs on the shell thread and owns the
    /// mouse until the drop.
    ///
    /// `app` is the app whose shortcut is carried (none for a recommended file). THE DOCK IS STILL A TARGET: a drag that has left the
    /// panel is the system's, and the way from the panel to the dock's island runs over the desktop, so the system's drag cannot start
    /// only where the pointer is sure not to be going to the island. Instead the island is watched while the system's loop runs (it is
    /// lit while the pointer is over it, `poll_over_island`), and a drop that ends over it adds the app to the dock's pins as a drop
    /// from the panel would (the island's window takes the file drop as a copy and does nothing with it).
    ///
    /// The panel closes when something was dropped, and stays when the drag came to nothing or came back over the panel itself.
    fn hand_to_system(&mut self, files: Vec<PathBuf>, app: Option<AppEntry>, cx: &mut Context<Self>) {
        self.drag.handed_over = true;
        self.drag.carry = None;
        self.drag.press = None;
        self.drag.shortcut_task = None;
        self.drag.click.drag_ended();
        crate::dock::drop_hint::set(false, cx);
        let ui_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        unsafe {
            // (the window holds the mouse from the press; the system's loop takes it itself)
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        // (the pointer is read on the shell thread the moment the loop ends: it is where the drop was)
        let (tx, rx) = futures::channel::oneshot::channel::<(Result<u32, String>, Option<(i32, i32)>)>();
        rf_win::sta::run(move |owner| {
            let done = rf_win::dnd::drag_out_copy_only(owner, ui_thread, &files).map(|effect| effect.0).map_err(|e| format!("{e:#}"));
            let _ = tx.send((done, geo::cursor()));
        });
        // the island is lit while the pointer is over it (an app only: a file has nothing to do there)
        self.drag.poll = app.is_some().then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    if !this.update(cx, |me, cx| me.poll_over_island(cx)).unwrap_or(false) {
                        break;
                    }
                }
            })
        });
        let hwnd = self.hwnd;
        cx.spawn(async move |this, cx| {
            let answer = rx.await;
            let _ = this.update(cx, |me, cx| {
                // (also when the shell thread's job was dropped without an answer: a hand-over that never ends would leave the panel with a
                // dead keyboard and no drag)
                me.drag.end_hand_over();
                crate::dock::drop_hint::set(false, cx);
                let Ok((done, at)) = answer else {
                    log::warn!("start drag: the shell thread gave no answer to the file drag");
                    cx.notify();
                    return;
                };
                match done {
                    Ok(effect) => {
                        let over_panel = window_rect(hwnd).zip(at).is_some_and(|(r, (x, y))| r.contains(x, y));
                        let on_island = on_island(at, cx);
                        // 0: nothing took it (Escape, or no target): nothing happened
                        if effect != 0 && !over_panel {
                            if let (true, Some(app)) = (on_island, app) {
                                crate::dock::own_pins::pin_start_app(app, cx);
                            }
                            me.finish(cx);
                        }
                    }
                    Err(e) => log::warn!("start drag: the file drag failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// While the system's loop runs with an app's shortcut: the island is lit when the pointer is over it. False when the loop is over.
    fn poll_over_island(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.drag.handed_over {
            return false;
        }
        crate::dock::drop_hint::set(on_island(geo::cursor(), cx), cx);
        true
    }

    // ------------------------------------------------------------------ drawing: the held thing

    /// The held app following the pointer: a tile's worth for a pinned app, a row's worth for an app of the list.
    pub(super) fn render_ghost(&mut self, pal: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (picked, pos, grab) = {
            let c = self.drag.carry.as_ref()?;
            (c.picked.clone(), c.pos, c.grab)
        };
        let app = picked.app()?.clone();
        let (left, top) = (pos.x - grab.x, pos.y - grab.y);
        let ghost = match picked {
            Picked::Pin { .. } => {
                let icon = self.app_picture(&app, page::TILE_ICON, pal, cx);
                v_flex()
                    .absolute()
                    .left(left)
                    .top(top)
                    .w(px(CELL_W))
                    .h(px(page::TILE_H))
                    .items_center()
                    .pt(px(12.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .bg(pal.pressed_fill().hsla())
                    .child(icon)
                    .child(div().w_full().h(px(30.)).px(px(6.)).text_size(px(12.)).line_height(px(15.)).text_center().line_clamp(2).child(app.name.clone()))
            }
            _ => {
                let icon = self.app_picture(&app, 24.0, pal, cx);
                h_flex()
                    .absolute()
                    .left(left)
                    .top(top)
                    .w(px(GHOST_ROW.0))
                    .h(px(GHOST_ROW.1))
                    .px(px(10.))
                    .gap_3()
                    .items_center()
                    .rounded(px(8.))
                    .bg(pal.pressed_fill().hsla())
                    .child(icon)
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(app.name.clone()))
            }
        };
        Some(ghost.opacity(0.95).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{COPIES_KEPT, COPY_FOLDER, DragState, GHOST_ROW, POLL, Picked, Press, Task, copy_root, point, px, rules};

    #[test]
    fn the_copies_of_shortcuts_live_in_the_temp_folder_and_are_kept_long_enough_for_a_slow_target() {
        assert!(copy_root().starts_with(std::env::temp_dir()));
        assert_eq!(copy_root().file_name().unwrap(), COPY_FOLDER);
        assert!(COPIES_KEPT.as_secs() >= 600);
    }

    #[test]
    fn a_lost_pointer_is_found_well_within_a_second() {
        // the quiet that is waited for plus the looks
        let at_most = rules::LostPointer::QUIET_MS + POLL.as_millis() as u64 * u64::from(rules::LostPointer::LOOKS);
        assert!(at_most <= 1000, "{at_most} ms");
        assert!(GHOST_ROW.0 > GHOST_ROW.1);
    }

    #[test]
    fn a_hand_over_whose_end_never_came_is_forgotten_when_the_panel_opens_again() {
        let mut d = DragState { handed_over: true, ..Default::default() };
        // (the keyboard and new drags are refused while a hand-over is going; it was the bug that nothing ever cleared it)
        d.click.drag_ended();
        d.reset_open();
        assert!(!d.handed_over);
        assert!(d.click.allow(), "and the silenced click of the last drag is forgotten too");
    }

    #[test]
    fn the_end_of_the_systems_loop_frees_the_state_even_when_it_gave_no_answer() {
        let mut d = DragState { handed_over: true, ..Default::default() };
        d.end_hand_over();
        assert!(!d.handed_over);
    }

    #[test]
    fn closing_the_panel_does_not_cancel_a_drag_that_is_in_the_systems_loop() {
        // (the island watch is the poll task: it must stay, and nothing is reported as cancelled)
        let mut d = DragState { handed_over: true, poll: Some(Task::ready(())), ..Default::default() };
        assert!(!d.cancel());
        assert!(d.handed_over && d.poll.is_some(), "the island watch goes on until the loop's end");
        // with no hand-over and nothing going on, nothing was cancelled either, and no click is silenced
        d.end_hand_over();
        assert!(d.poll.is_none());
        assert!(!d.cancel());
        assert!(d.click.allow());
    }

    #[test]
    fn a_press_that_is_put_back_silences_the_release_that_follows() {
        let press = Press { picked: Picked::Rec { path: "x".into() }, at: point(px(0.), px(0.)), grab: point(px(0.), px(0.)) };
        let mut d = DragState { press: Some(press), ..Default::default() };
        assert!(d.cancel());
        assert!(d.press.is_none());
        assert!(!d.click.allow(), "the release is not a click");
        assert!(d.click.allow());
    }
}
