//! The dock's app menu (shell plan increment B8): a right click on an app's icon in the middle island opens OUR menu on glass above the
//! icon. Top to bottom: the jump-list sections of the app (`jump_source`), then the fixed part: the app's name (starts a new instance),
//! "Dock'a sabitle" / "Dock'tan kaldır" (the dock's own pin list, `own_pins`) and, for a running app, "Pencereyi kapat" / "Tüm
//! pencereleri kapat (n)". The rows, their order and the keyboard's walk are `rf_win::dock::appmenu` (pure, tested); this file is the
//! window and what the rows do.
//!
//! THE MENU OPENS AT ONCE with the fixed part. The sections are asked for on a worker thread afterwards and appear when they arrive (the
//! window grows upwards: it is anchored by its bottom edge); a highlighted row stays highlighted. Nothing waits for the jump lists.
//!
//! THE WINDOW is made when the first right click needs it and kept hidden afterwards (`popup_win`). Like the dock's panels it IS
//! activated: it takes the keyboard (arrows, Home, End, Enter, Escape) and gives it back to the window that had it when it is put away
//! with Escape. It closes on Escape, a mouse button going down anywhere outside it (a right click on another icon closes it and the
//! click then opens that icon's menu), another window taking the foreground, the dock hiding for a full-screen application, the
//! monitor or the work area changing, Alt+F4 (only put away), and when a row has been chosen. One menu at a time: a right click on
//! another icon while it is open moves it. While it is open the hover previews are blocked.
//!
//! ALL WIN32 CALLS THAT SHOW, MOVE OR HIDE THE WINDOW run in a task of the foreground executor, outside any update (see `host.rs`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::appmenu::{self as model, Fixed, Item, Menu, RowId, Shape};
use rf_win::dock::layout::Rect;
use rf_win::dock::panel::{self as geo, CloseCause};
use rf_win::dock::preview as place;
use rf_win::dock::winops::{self, WinRef};
use rf_win::dock::{DockApp, ownpins};

use super::backdrop::IslandGlass;
use super::host;
use super::jump_source::{self, JumpApp, Section};
use super::look::Palette;
use super::popup_win;
use super::{icons, plan};
use crate::glass_watch::{self, PopupGlass};
use crate::t;

/// How often an open menu looks at the foreground and the mouse (the same as the dock's panels).
const WATCH_EVERY: Duration = Duration::from_millis(50);

/// What a right click asks for.
pub struct MenuRequest {
    pub app: DockApp,
    /// The app's picture, if the island has it by now (for the name row).
    pub icon: Option<Arc<RenderImage>>,
    /// The icon's centre, device px, screen coordinates.
    pub anchor_x: f32,
    /// The middle island's glass (its tone is the menu's).
    pub glass: Rc<RefCell<IslandGlass>>,
}

/// Opens the menu for an app (or moves the open one to it). Safe to call from a click handler: the work is done from the message loop.
pub fn open(req: MenuRequest, cx: &mut App) {
    let started = Instant::now();
    cx.spawn(async move |cx| {
        cx.update(|cx| open_now(req, started, cx));
    })
    .detach();
}

/// The menu is open (or easing out).
pub fn is_open(cx: &App) -> bool {
    cx.try_global::<MenuHost>().is_some_and(|h| h.open)
}

/// The toolkit's id of the menu's window, when it has been made.
pub fn window_ids(cx: &App) -> Vec<WindowId> {
    cx.try_global::<MenuHost>().and_then(|h| h.made.as_ref().map(|m| m.window)).into_iter().collect()
}

/// The dock ends: the window goes with it.
pub fn shutdown(cx: &mut App) {
    if !cx.has_global::<MenuHost>() {
        return;
    }
    let host = cx.remove_global::<MenuHost>();
    if let Some(m) = host.made {
        let _ = m.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

// ---------------------------------------------------------------- the host side

struct Made {
    handle: WindowHandle<MenuView>,
    hwnd: isize,
    window: WindowId,
}

/// What the menu was placed from: when it changes under an open menu the menu would be left behind.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Basis {
    scale: f32,
    work: Rect,
    glass_top: i32,
}

#[derive(Default)]
struct MenuHost {
    made: Option<Made>,
    open: bool,
    /// Bumped at every open and close: a task of an earlier state sees that and does nothing.
    serial: u64,
    rect: Rect,
    basis: Option<Basis>,
    anchor_x: i32,
    previous: isize,
    opened_at: Option<Instant>,
    been_front: bool,
    mouse_was_down: bool,
    watcher: Option<Task<()>>,
}

impl Global for MenuHost {}

fn host_g(cx: &mut App) -> &mut MenuHost {
    if !cx.has_global::<MenuHost>() {
        cx.set_global(MenuHost::default());
    }
    cx.global_mut::<MenuHost>()
}

fn on_window_close(cx: &mut App) {
    close(CloseCause::WindowClose, cx);
}

/// The window, made now if it was not (the first right click). `None` when it cannot be made.
fn ensure_window(island: &Rc<RefCell<IslandGlass>>, cx: &mut App) -> Option<(WindowHandle<MenuView>, isize, bool)> {
    if let Some(m) = cx.try_global::<MenuHost>().and_then(|h| h.made.as_ref()) {
        return Some((m.handle, m.hwnd, false));
    }
    let (scale, _, glass) = popup_win::anchor_basis(cx)?;
    let display = cx.primary_display().map(|d| d.id());
    let isl = island.clone();
    let handle = popup_win::open_hidden((model::WIDTH, 160.0), display, glass, scale, on_window_close, move |window, cx| MenuView::new(isl, window, cx), cx)?;
    let hwnd = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).ok().filter(|h| *h != 0)?;
    host_g(cx).made = Some(Made { handle, hwnd, window: handle.window_id() });
    Some((handle, hwnd, true))
}

fn open_now(req: MenuRequest, started: Instant, cx: &mut App) {
    if host::is_fullscreen(cx) {
        return;
    }
    let Some((scale, work, glass)) = popup_win::anchor_basis(cx) else { return };
    let Some((handle, hwnd, made_now)) = ensure_window(&req.glass, cx) else {
        log::warn!("dock: cannot make the app menu's window");
        return;
    };
    let dark = req.glass.borrow().dark();
    let anchor_x = req.anchor_x.round() as i32;
    let serial = {
        let was_open = cx.try_global::<MenuHost>().is_some_and(|h| h.open);
        let front = rf_win::dock::foreground_root();
        let p = host_g(cx);
        p.serial += 1;
        // (a menu that is already open keeps the window that had the keyboard before it; one still easing out has the menu itself in
        // front, which is nothing to remember)
        if !was_open && p.made.as_ref().is_none_or(|m| m.hwnd != front) {
            p.previous = front;
        }
        p.open = true;
        p.opened_at = Some(Instant::now());
        p.been_front = false;
        p.mouse_was_down = geo::any_button_down();
        p.basis = Some(Basis { scale, work, glass_top: glass.top });
        p.anchor_x = anchor_x;
        p.serial
    };
    // the previews step aside while a menu is open
    super::hover_preview::block(true, cx);
    let jump = JumpApp::of(&req.app);
    let size = handle
        .update(cx, |v, window, cx| {
            v.on_open(req.app, req.icon, jump.clone(), serial, window, cx);
            (model::WIDTH, v.height())
        })
        .unwrap_or((model::WIDTH, 160.0));
    let rect = place::above_icon(anchor_x, glass.top, size, scale, host::size_on(host::active(cx), cx), work);
    host_g(cx).rect = rect;
    // show, bring to the front, then glass (in this order: the engine attaches to a window that is on the screen)
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        geo::show_at(hwnd, rect);
        // (the quiet kind: no tap of the Alt key into the window that is in front; the click that opened the menu was ours)
        rf_win::window::bring_to_front_quietly(hwnd);
        cx.update(|cx| {
            let _ = handle.update(cx, |v, window, cx| v.on_shown(dark, window, cx));
            log::info!("dock: app menu shown in {:?} from the click at x={anchor_x}{}", started.elapsed(), if made_now { " (window made for it)" } else { "" });
        });
    })
    .detach();
    // the jump lists, off the UI thread; the menu is open already
    let named = jump.clone();
    cx.spawn(async move |cx| {
        let sections = cx.background_executor().spawn(async move { jump_source::sections(&jump) }).await;
        cx.update(|cx| sections_arrived(serial, sections, cx));
    })
    .detach();
    // the app's name as the Start panel shows it, asked apart from the lists (the shell thread answers one at a time, and neither waits
    // for the other); nothing is asked for a pinned app
    if !named.pinned {
        cx.spawn(async move |cx| {
            let name = cx.background_executor().spawn(async move { jump_source::display_name(&named) }).await;
            if let Some(name) = name {
                cx.update(|cx| name_arrived(serial, name, cx));
            }
        })
        .detach();
    }
    // the watch of an open menu
    let watcher = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(WATCH_EVERY).await;
            if !cx.update(|cx| watch_once(serial, cx)) {
                break;
            }
        }
    });
    host_g(cx).watcher = Some(watcher);
}

/// The sections arrived: the menu gets them and, being anchored by its bottom edge, grows upwards.
fn sections_arrived(serial: u64, sections: Vec<Section>, cx: &mut App) {
    let Some(h) = cx.try_global::<MenuHost>() else { return };
    let (Some(made), true) = (h.made.as_ref(), h.open && h.serial == serial) else { return };
    let (handle, hwnd) = (made.handle, made.hwnd);
    let (basis, anchor_x) = (h.basis, h.anchor_x);
    let Some(basis) = basis else { return };
    // (the room above the dock: a long jump list is cut to what fits, so that the fixed rows at the bottom are never cut off)
    let room = place::room_above(basis.glass_top, basis.scale, host::size_on(host::active(cx), cx), basis.work);
    let Ok(height) = handle.update(cx, |v, _, cx| {
        v.set_sections(sections, room, cx);
        v.height()
    }) else {
        return;
    };
    let rect = place::above_icon(anchor_x, basis.glass_top, (model::WIDTH, height), basis.scale, host::size_on(host::active(cx), cx), basis.work);
    if cx.global::<MenuHost>().rect == rect {
        return;
    }
    cx.global_mut::<MenuHost>().rect = rect;
    cx.spawn(async move |_| {
        geo::show_at(hwnd, rect);
    })
    .detach();
}

/// The shell named the app: the bold line changes (its width is fixed, so the menu does not move).
fn name_arrived(serial: u64, name: String, cx: &mut App) {
    let Some(h) = cx.try_global::<MenuHost>() else { return };
    let (Some(made), true) = (h.made.as_ref(), h.open && h.serial == serial) else { return };
    let handle = made.handle;
    let _ = handle.update(cx, |v, _, cx| v.set_display_name(name, cx));
}

/// Whether what the menu was placed from is not what the dock has now.
fn basis_changed(then: &Basis, now: Option<Basis>) -> bool {
    now.is_none_or(|n| n != *then)
}

/// One look of the open menu at the foreground and the mouse. False when this watch is over (the menu closed or reopened).
fn watch_once(serial: u64, cx: &mut App) -> bool {
    let Some(h) = cx.try_global::<MenuHost>() else { return false };
    let (Some(made), true) = (h.made.as_ref(), h.open && h.serial == serial) else { return false };
    let hwnd = made.hwnd;
    let (rect, opened_at, been_front, was_down, then) = (h.rect, h.opened_at, h.been_front, h.mouse_was_down, h.basis);
    if host::is_fullscreen(cx) {
        close(CloseCause::FullScreen, cx);
        return false;
    }
    let now = popup_win::anchor_basis(cx).map(|(scale, work, glass)| Basis { scale, work, glass_top: glass.top });
    if then.is_some_and(|t| basis_changed(&t, now)) {
        close(CloseCause::LayoutChanged, cx);
        return false;
    }
    let fg = rf_win::dock::foreground_root();
    let front = fg == hwnd;
    let down = geo::any_button_down();
    // (the dock's own island is not exempt: a click on another icon closes this menu, and its mouse-up opens that icon's)
    let outside = geo::cursor().is_some_and(|c| geo::outside_click(was_down, down, c, rect, Rect::default()));
    {
        let h = cx.global_mut::<MenuHost>();
        h.been_front |= front;
        h.mouse_was_down = down;
    }
    if outside {
        close(CloseCause::ClickOutside, cx);
        return false;
    }
    let since = opened_at.map_or(0, |t| t.elapsed().as_millis() as u64);
    if geo::lost_foreground(hwnd, fg, been_front || front, since) {
        close(CloseCause::ForegroundLost, cx);
        return false;
    }
    true
}

/// Puts the menu away: it eases out over 120 ms (at once with animations off), then the window is hidden and the glass taken off.
/// Does nothing when none is open.
pub fn close(cause: CloseCause, cx: &mut App) {
    let Some(h) = cx.try_global::<MenuHost>() else { return };
    let (true, Some(made)) = (h.open, h.made.as_ref()) else { return };
    let (hwnd, handle, previous) = (made.hwnd, made.handle, h.previous);
    let animate = crate::settings::animations();
    {
        let h = cx.global_mut::<MenuHost>();
        h.open = false;
        h.serial += 1;
        h.watcher = None;
    }
    let _ = handle.update(cx, |v, window, cx| v.on_close(window, cx));
    let restore = cause.restores_foreground();
    cx.spawn(async move |cx| {
        if animate {
            cx.background_executor().timer(Duration::from_millis(u64::from(geo::ANIM_MS) + 20)).await;
        }
        // opened again meanwhile: leave everything as it is
        if cx.update(|cx| is_open(cx)) {
            return;
        }
        let fg = rf_win::dock::foreground_root();
        geo::hide(hwnd);
        cx.update(|cx| {
            let _ = handle.update(cx, |v, window, cx| v.on_hidden(window, cx));
            super::hover_preview::block(false, cx);
        });
        if restore && let Some(target) = geo::restore_to(previous, fg, hwnd, geo::exists(previous)) {
            geo::give_back(target);
        }
    })
    .detach();
}

// ---------------------------------------------------------------- the view

/// What the menu is open for.
struct Active {
    app: DockApp,
    /// The app's windows as seen when the menu opened (handle and process): "close" acts on these only while they are still the same
    /// windows, however long the menu has been open.
    windows: Vec<WinRef>,
    jump: JumpApp,
    icon: Option<Arc<RenderImage>>,
    /// A better name for the bold line than the dock's, once the shell has answered (see `jump_source::display_name`).
    display_name: Option<String>,
    menu: Menu,
    /// The sections that have lines (the menu's indices count these).
    sections: Vec<Section>,
}

pub struct MenuView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// On the screen or easing out (false while hidden: nothing is built then).
    shown: bool,
    closing: bool,
    anim: Option<(Instant, bool)>,
    focus: FocusHandle,
    active: Option<Active>,
    /// The pictures of jump-list lines, by `plan::icon_key`.
    entry_icons: HashMap<String, Arc<RenderImage>>,
    /// Lines whose icon could not be drawn: they wear the app's icon instead.
    failed: HashSet<String>,
    asked: HashSet<String>,
    _tasks: Vec<Task<()>>,
}

impl MenuView {
    fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        MenuView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            focus: cx.focus_handle(),
            active: None,
            entry_icons: HashMap::new(),
            failed: HashSet::new(),
            asked: HashSet::new(),
            _tasks: Vec::new(),
        }
    }

    /// About to be shown for this app: the fixed part, easing in.
    fn on_open(&mut self, app: DockApp, icon: Option<Arc<RenderImage>>, jump: JumpApp, _serial: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let windows = winops::refs(&app.windows);
        // (whether there is a process of the app's own to end: a few fast questions about its windows, none of which waits for it)
        let can_end = !winops::end_task_processes(&windows).is_empty();
        let on_disk = app.launch.as_ref().is_some_and(|l| l.file().is_some());
        let fixed = model::fixed_rows(app.launch.is_some(), on_disk, ownpins::pin_from_app(&app).is_some(), app.pinned, app.windows.len(), can_end);
        // (the monitors the window used last is not on: two quick questions of Windows, none of which waits for the program. An app
        // with no window is offered every monitor, if it is started from a file: its new window is told by its program)
        let monitors = match windows.first() {
            Some(w) => rf_win::dock::winmove::targets(&rf_win::dock::winmove::screens(), rf_win::dock::monitors::monitor_of_window(w.hwnd)),
            None if on_disk => rf_win::dock::winmove::targets(&rf_win::dock::winmove::screens(), None),
            None => Vec::new(),
        };
        // rustyfiles' Start: the app's pin there too, when Start is ours and the app is in its "All apps" (issue 174)
        let fixed = model::with_start_row(fixed, super::start::dock_pin_state(&app, cx));
        let fixed = model::with_window_rows(fixed, &monitors);
        self.active = Some(Active { app, windows, jump, icon, display_name: None, menu: Menu::new(fixed), sections: Vec::new() });
        self.shown = true;
        self.closing = false;
        // (a menu that moves from one icon to another does not ease in again from nothing)
        self.anim = Some((Instant::now(), true));
        self._tasks.clear();
        self.asked.clear();
        cx.notify();
    }

    fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel(self.hwnd, dark));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        cx.notify();
    }

    /// Hidden: the glass comes off and what the menu held is let go.
    fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.closing = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        self.active = None;
        self._tasks.clear();
        self.asked.clear();
        self.entry_icons.clear();
        self.failed.clear();
        cx.notify();
    }

    /// The shell answered with a better name for the bold line.
    fn set_display_name(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(a) = self.active.as_mut() else { return };
        a.display_name = Some(name);
        cx.notify();
    }

    /// The menu's height now, logical px.
    fn height(&self) -> f32 {
        self.active.as_ref().map_or(2.0 * model::PAD, |a| a.menu.height())
    }

    /// The sections arrived; `room` is the height the menu may have (logical px). Lines beyond what fits are not shown (`model::fit_entries`).
    fn set_sections(&mut self, sections: Vec<Section>, room: f32, cx: &mut Context<Self>) {
        let Some(a) = self.active.as_mut() else { return };
        let mut sections: Vec<Section> = sections.into_iter().filter(|s| !s.entries.is_empty()).collect();
        let shape = |s: &Section| Shape { titled: s.title.as_deref().is_some_and(|t| !t.trim().is_empty()), entries: s.entries.len() };
        let shapes: Vec<Shape> = sections.iter().map(shape).collect();
        for (s, n) in sections.iter_mut().zip(model::fit_entries(&shapes, a.menu.fixed(), room)) {
            s.entries.truncate(n);
        }
        sections.retain(|s| !s.entries.is_empty());
        a.menu.set_sections(sections.iter().map(shape).collect());
        a.sections = sections;
        self.load_entry_icons(cx);
        cx.notify();
    }

    /// Asks for the icon of every line that has one, on worker threads.
    fn load_entry_icons(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.active.as_ref() else { return };
        let px = (16.0 * host::env_on(host::active(cx), cx).scale).ceil() as u32;
        let wanted: Vec<(String, rf_win::dock::IconSource)> = a
            .sections
            .iter()
            .flat_map(|s| &s.entries)
            .filter_map(|e| e.icon.clone())
            .map(|src| (plan::icon_key(&src), src))
            .filter(|(key, _)| !self.asked.contains(key))
            .collect();
        for (key, src) in wanted {
            if !self.asked.insert(key.clone()) {
                continue;
            }
            let task = cx.spawn(async move |this, cx| {
                let bgra = cx.background_executor().spawn(async move { icons::load_bgra(&[src], px) }).await;
                let img = bgra.and_then(icons::to_image);
                let _ = this.update(cx, |me, cx| {
                    match img {
                        Some(img) => {
                            me.entry_icons.insert(key, img);
                        }
                        // (it could not be drawn: the line wears the app's icon, not a hole)
                        None => {
                            me.failed.insert(key);
                        }
                    }
                    cx.notify();
                });
            });
            self._tasks.push(task);
        }
    }

    fn live(&self) -> bool {
        self.shown && !self.closing
    }

    fn hover(&mut self, id: RowId, on: bool, cx: &mut Context<Self>) {
        let Some(a) = self.active.as_mut() else { return };
        if on {
            a.menu.hover(Some(id));
        } else if a.menu.highlighted() == Some(id) {
            a.menu.hover(None);
        } else {
            return;
        }
        cx.notify();
    }

    /// A row was chosen (a click, or Enter on the highlighted one): do it, and put the menu away.
    fn choose(&mut self, id: RowId, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(a) = self.active.as_ref() else { return };
        match id {
            RowId::Entry(s, e) => {
                if let Some(entry) = a.sections.get(s).and_then(|x| x.entries.get(e)) {
                    let (jump, action) = (a.jump.clone(), entry.action.clone());
                    // (it can wait on the shell or on the other program: never on the UI thread)
                    std::thread::spawn(move || {
                        if let Err(err) = jump_source::run(&jump, &action) {
                            log::warn!("dock: a jump-list entry failed: {err:#}");
                        }
                    });
                }
            }
            RowId::Fixed(Fixed::Open) => super::actions::middle_click_app(&a.app),
            RowId::Fixed(Fixed::RunAsAdmin) => {
                if let Some(file) = a.app.launch.as_ref().and_then(|l| l.file()).map(|p| p.to_path_buf()) {
                    // Windows shows its consent prompt: the user asked for it with this row. (A shortcut brings its arguments
                    // along; a bare program is started without any.)
                    std::thread::spawn(move || {
                        if let Err(e) = rf_win::dock::startapps::launch_file_as_admin(&file)
                            && !rf_win::dock::startapps::is_cancelled(&e)
                        {
                            log::warn!("dock: starting as administrator failed: {e:#}");
                        }
                    });
                }
            }
            RowId::Fixed(Fixed::OpenLocation) => {
                if let Some(file) = a.app.launch.as_ref().and_then(|l| l.file()).map(|p| p.to_path_buf()) {
                    // (reading a shortcut asks the shell: not on the UI thread)
                    std::thread::spawn(move || super::actions::run_rustyfiles(super::actions::show_in_folder_args(&location_of(&file))));
                }
            }
            RowId::Fixed(Fixed::Properties) => {
                if let Some(file) = a.app.launch.as_ref().and_then(|l| l.file()).map(|p| p.to_path_buf()) {
                    std::thread::spawn(move || {
                        if let Err(e) = rf_win::shell::properties(&file) {
                            log::warn!("dock: the properties could not be opened: {e:#}");
                        }
                    });
                }
            }
            RowId::Fixed(Fixed::Pin) => super::own_pins::set_pinned(a.app.clone(), true, cx),
            RowId::Fixed(Fixed::Unpin) => super::own_pins::set_pinned(a.app.clone(), false, cx),
            RowId::Fixed(Fixed::PinStart) => super::start::pin_dock_app(a.app.clone(), true, cx),
            RowId::Fixed(Fixed::UnpinStart) => super::start::pin_dock_app(a.app.clone(), false, cx),
            RowId::Fixed(Fixed::MoveToMonitor(number)) => {
                // (the window used last, as "close window" takes it; moving sends the program messages: never on the UI thread)
                if let Some(w) = a.windows.first().copied() {
                    std::thread::spawn(move || {
                        if rf_win::dock::winmove::move_to_screen(w, number) {
                            rf_win::dock::launch::activate(w.hwnd);
                        } else {
                            log::info!("dock: the window was not moved to monitor {number} (it is gone, or Windows refused)");
                        }
                    });
                } else if let Some(launch) = a.app.launch.clone() {
                    // Not running: started as always, and its new window is moved when it shows (a program cannot be told where
                    // to open). The windows that are there now are noted first, to tell the new one.
                    std::thread::spawn(move || {
                        use rf_win::dock::winmove;
                        let program = winmove::program_of(&launch);
                        let known = winmove::known_windows();
                        if let Err(e) = rf_win::dock::launch::start(&launch) {
                            log::warn!("dock: starting the app for monitor {number} failed: {e:#}");
                            return;
                        }
                        let found = program.and_then(|p| winmove::wait_for_new_window(&p, &known, std::time::Duration::from_secs(20)));
                        match found {
                            Some(w) if winmove::move_to_screen(w, number) => rf_win::dock::launch::activate(w.hwnd),
                            Some(_) => log::info!("dock: the new window was not moved to monitor {number} (Windows refused)"),
                            None => log::info!("dock: no new window of the started program was seen; it stays where it opened"),
                        }
                    });
                }
            }
            RowId::Fixed(Fixed::FitToScreen) => {
                if let Some(w) = a.windows.first().copied() {
                    std::thread::spawn(move || {
                        if rf_win::dock::winmove::fit_to_screen(w) {
                            rf_win::dock::launch::activate(w.hwnd);
                        }
                    });
                }
            }
            RowId::Fixed(Fixed::CloseWindow) => {
                // (the one used last: the first of the list; if it is not the window it was any more, nothing is closed)
                if let Some(w) = a.windows.first() {
                    winops::close_window(*w);
                }
            }
            RowId::Fixed(Fixed::CloseAll(_)) => {
                for w in &a.windows {
                    winops::close_window(*w);
                }
            }
            RowId::Fixed(Fixed::EndTask) => {
                // The user chose "end task": the app's processes are ended without being asked (off the UI thread; the windows are
                // checked again there, so a handle that went to another program meanwhile ends nothing).
                let (windows, name) = (a.windows.clone(), a.app.name.clone());
                std::thread::spawn(move || {
                    let (ended, refused) = winops::end_task(&windows);
                    log::info!("dock: end task for {name:?}: {ended} process(es) ended, {refused} refused");
                });
            }
        }
        self.closing = true;
        cx.defer(|cx| close(CloseCause::Chosen, cx));
    }

    fn key(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        match key {
            "escape" => {
                self.closing = true;
                cx.defer(|cx| close(CloseCause::Escape, cx));
                return;
            }
            "enter" | "space" => {
                if let Some(id) = self.active.as_ref().and_then(|a| a.menu.highlighted()) {
                    self.choose(id, cx);
                }
                return;
            }
            _ => {}
        }
        let Some(a) = self.active.as_mut() else { return };
        match key {
            "down" => a.menu.step(1),
            "up" => a.menu.step(-1),
            "tab" => a.menu.step(if shift { -1 } else { 1 }),
            "home" => a.menu.home(),
            "end" => a.menu.end(),
            _ => return,
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ drawing

    fn row_content(&self, id: RowId) -> (Option<Arc<RenderImage>>, Option<L>, SharedString) {
        let Some(a) = self.active.as_ref() else { return (None, None, SharedString::default()) };
        match id {
            RowId::Entry(s, e) => {
                let entry = a.sections.get(s).and_then(|x| x.entries.get(e));
                let own = entry.and_then(|en| en.icon.as_ref()).map(plan::icon_key);
                let loaded = own.as_ref().and_then(|k| self.entry_icons.get(k)).cloned();
                let failed = own.as_ref().is_some_and(|k| self.failed.contains(k));
                let icon = match entry_picture(own.is_some(), loaded.is_some(), failed) {
                    EntryPicture::Own => loaded,
                    EntryPicture::App => a.icon.clone(),
                    EntryPicture::Waiting => None,
                };
                (icon, None, entry.map(|en| en.title.clone()).unwrap_or_default().into())
            }
            RowId::Fixed(f) => {
                let (glyph, label) = fixed_label(f, a.display_name.as_deref().unwrap_or(&a.app.name));
                // the name row wears the app's own picture
                if f == Fixed::Open { (a.icon.clone(), Some(L::Play), label.into()) } else { (None, glyph, label.into()) }
            }
        }
    }

    fn render_row(&self, id: RowId, hot: bool, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let (hover, text) = (pal.hover_fill().hsla(), pal.text().hsla());
        let (image, glyph, label) = self.row_content(id);
        let name_row = id == RowId::Fixed(Fixed::Open);
        let lead: AnyElement = match (image, glyph) {
            (Some(img), _) => gpui_kit::img(img).size(px(18.)).into_any_element(),
            (None, Some(g)) => Icon::new(g).size(px(16.)).text_color(text).into_any_element(),
            (None, None) => div().size(px(16.)).into_any_element(),
        };
        h_flex()
            .id(match id {
                RowId::Entry(s, e) => ElementId::from(("menu-entry", s * 1000 + e)),
                RowId::Fixed(_) => ElementId::from(SharedString::from(format!("menu-fixed-{id:?}"))),
            })
            .h(px(model::ROW_H))
            .flex_shrink_0()
            .px(px(10.))
            .gap(px(10.))
            .items_center()
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.))
            .when(name_row, |d| d.font_weight(FontWeight::SEMIBOLD))
            .when(hot, move |d| d.bg(hover))
            .child(div().size(px(18.)).flex_shrink_0().flex().items_center().justify_center().child(lead))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .on_hover(cx.listener(move |this, on: &bool, _, cx| this.hover(id, *on, cx)))
            .on_click(cx.listener(move |this, _, _, cx| this.choose(id, cx)))
            .into_any_element()
    }
}

/// Which picture a jump-list line wears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryPicture {
    /// Its own, which has been drawn.
    Own,
    /// The app's: the line has no icon of its own to draw, or the reader could not draw it.
    App,
    /// Its own, which is still being read: nothing yet (a flash of the app's icon would be replaced a moment later).
    Waiting,
}

/// `declared`: the entry names an icon; `loaded`: that icon has been drawn; `failed`: it could not be.
fn entry_picture(declared: bool, loaded: bool, failed: bool) -> EntryPicture {
    match (declared, loaded, failed) {
        (false, _, _) | (true, false, true) => EntryPicture::App,
        (true, true, _) => EntryPicture::Own,
        (true, false, false) => EntryPicture::Waiting,
    }
}

/// The glyph and the text of a fixed row. The name row's text is the app's name.
fn fixed_label(f: Fixed, app_name: &str) -> (Option<L>, String) {
    match f {
        Fixed::Open if app_name.trim().is_empty() => (Some(L::Play), t!("Uygulamayı başlat").to_string()),
        Fixed::Open => (Some(L::Play), app_name.to_string()),
        Fixed::RunAsAdmin => (Some(L::ShieldCheck), t!("Yönetici olarak çalıştır").to_string()),
        Fixed::OpenLocation => (Some(L::FolderOpen), t!("Dosya konumunu aç").to_string()),
        Fixed::Properties => (Some(L::Wrench), t!("Özellikler").to_string()),
        Fixed::MoveToMonitor(n) => (Some(L::Monitor), format!("{}: {n}", t!("Monitörde aç"))),
        Fixed::FitToScreen => (Some(L::Maximize2), t!("Ekrana sığdır").to_string()),
        Fixed::Pin => (Some(L::Pin), t!("Dock'a sabitle").to_string()),
        Fixed::Unpin => (Some(L::PinOff), t!("Dock'tan kaldır").to_string()),
        Fixed::PinStart => (Some(L::Pin), t!("Başlat'a sabitle").to_string()),
        Fixed::UnpinStart => (Some(L::PinOff), t!("Başlat'tan kaldır").to_string()),
        Fixed::CloseWindow => (Some(L::X), t!("Pencereyi kapat").to_string()),
        Fixed::CloseAll(n) => (Some(L::X), format!("{} ({n})", t!("Tüm pencereleri kapat"))),
        Fixed::EndTask => (Some(L::CircleSlash), t!("Görevi sonlandır").to_string()),
    }
}

impl Render for MenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown || self.active.is_none() {
            return div().size_full().into_any_element();
        }
        let (pal, _) = host::frame_palette(&self.island, window);
        let dark = pal.mix < 0.5;
        let fill = glass_watch::panel_fill(dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let frame = match self.anim {
            Some((t, opening)) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
            None => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
        };
        if !frame.done {
            window.request_animation_frame();
        }
        let (items, hi, titles): (Vec<Item>, Option<RowId>, Vec<SharedString>) = match self.active.as_ref() {
            Some(a) => (a.menu.items(), a.menu.highlighted(), a.sections.iter().map(|s| s.title.clone().unwrap_or_default().into()).collect()),
            None => (Vec::new(), None, Vec::new()),
        };
        let secondary = pal.text_secondary().hsla();
        let line = pal.separator().hsla();
        let mut column = v_flex().size_full().p(px(model::PAD)).opacity(frame.opacity).mt(px(frame.drop_px));
        for item in items {
            column = column.child(match item {
                Item::Row(id) => self.render_row(id, hi == Some(id), pal, cx),
                Item::Header(s) => div()
                    .h(px(model::HEADER_H))
                    .flex_shrink_0()
                    .px(px(10.))
                    .flex()
                    .items_end()
                    .pb(px(3.))
                    .text_xs()
                    .text_color(secondary)
                    .child(titles.get(s).cloned().unwrap_or_default())
                    .into_any_element(),
                Item::Separator => div()
                    .h(px(model::SEPARATOR_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(px(6.))
                    .child(div().h(px(1.)).w_full().bg(line))
                    .into_any_element(),
            });
        }
        div()
            .id("app-menu")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(pal.text().hsla())
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let key = ev.keystroke.key.as_str();
                if matches!(key, "escape" | "enter" | "space" | "down" | "up" | "tab" | "home" | "end") {
                    cx.stop_propagation();
                    this.key(key, ev.keystroke.modifiers.shift, cx);
                }
            }))
            .child(column)
            .into_any_element()
    }
}

/// The file "open file location" shows: a shortcut's target when it has one on disk (what the button of a shortcut's Properties
/// page opens), else the file itself.
fn location_of(file: &std::path::Path) -> std::path::PathBuf {
    let is_link = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
    let target = is_link.then(|| rf_win::shortcut::resolve(file).ok()).flatten().map(|s| s.target).filter(|t| !t.as_os_str().is_empty() && t.exists());
    target.unwrap_or_else(|| file.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{Basis, EntryPicture, Fixed, basis_changed, entry_picture, fixed_label};
    use rf_win::dock::layout::Rect;

    #[test]
    fn a_line_wears_its_own_icon_the_apps_when_it_has_none_or_it_cannot_be_drawn_and_nothing_while_it_loads() {
        assert_eq!(entry_picture(true, true, false), EntryPicture::Own);
        assert_eq!(entry_picture(true, false, false), EntryPicture::Waiting);
        assert_eq!(entry_picture(true, false, true), EntryPicture::App, "the reader gave nothing");
        assert_eq!(entry_picture(false, false, false), EntryPicture::App, "a task of a packaged app names no icon");
        assert_eq!(entry_picture(true, true, true), EntryPicture::Own, "a picture that arrived after a first failure is used");
    }

    #[test]
    fn the_menu_is_on_by_default_and_a_file_from_before_it_reads_as_on() {
        assert!(crate::settings::Settings::default().dock_app_menu);
        assert!(crate::settings::Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap().dock_app_menu);
        let off = crate::settings::Settings { dock_app_menu: false, ..Default::default() };
        let back = crate::settings::Settings::parse_lenient_pub(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_app_menu);
        assert!(crate::settings::Settings::parse_lenient_pub("dock_app_menu = 3\n").unwrap().dock_app_menu, "a wrong value falls back to the default");
    }

    #[test]
    fn the_fixed_rows_read_in_turkish_and_the_count_is_in_brackets() {
        assert_eq!(fixed_label(Fixed::Pin, "x").1, "Dock'a sabitle");
        assert_eq!(fixed_label(Fixed::Unpin, "x").1, "Dock'tan kaldır");
        assert_eq!(fixed_label(Fixed::CloseWindow, "x").1, "Pencereyi kapat");
        assert_eq!(fixed_label(Fixed::CloseAll(3), "x").1, "Tüm pencereleri kapat (3)");
        assert_eq!(fixed_label(Fixed::EndTask, "x").1, "Görevi sonlandır");
    }

    #[test]
    fn the_name_row_is_the_apps_name_or_a_plain_word_when_it_has_none() {
        assert_eq!(fixed_label(Fixed::Open, "Notepad").1, "Notepad");
        assert_eq!(fixed_label(Fixed::Open, "  ").1, "Uygulamayı başlat");
    }

    #[test]
    fn an_open_menu_is_left_behind_only_when_what_it_was_placed_from_changed() {
        let b = Basis { scale: 1.0, work: Rect::new(0, 0, 2560, 1392), glass_top: 1334 };
        assert!(!basis_changed(&b, Some(b)));
        assert!(basis_changed(&b, Some(Basis { glass_top: 1300, ..b })));
        assert!(basis_changed(&b, Some(Basis { scale: 1.25, ..b })));
        assert!(basis_changed(&b, Some(Basis { work: Rect::new(0, 0, 1920, 1032), ..b })));
        assert!(basis_changed(&b, None), "the dock has no layout any more");
    }
}
