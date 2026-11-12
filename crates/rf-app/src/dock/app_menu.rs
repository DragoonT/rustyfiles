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
use super::look::{Env, Palette};
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
    let rect = place::above_icon(anchor_x, glass.top, size, scale, work);
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
    cx.spawn(async move |cx| {
        let sections = cx.background_executor().spawn(async move { jump_source::sections(&jump) }).await;
        cx.update(|cx| sections_arrived(serial, sections, cx));
    })
    .detach();
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
    let room = place::room_above(basis.glass_top, basis.scale, basis.work);
    let Ok(height) = handle.update(cx, |v, _, cx| {
        v.set_sections(sections, room, cx);
        v.height()
    }) else {
        return;
    };
    let rect = place::above_icon(anchor_x, basis.glass_top, (model::WIDTH, height), basis.scale, basis.work);
    if cx.global::<MenuHost>().rect == rect {
        return;
    }
    cx.global_mut::<MenuHost>().rect = rect;
    cx.spawn(async move |_| {
        geo::show_at(hwnd, rect);
    })
    .detach();
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
            asked: HashSet::new(),
            _tasks: Vec::new(),
        }
    }

    /// About to be shown for this app: the fixed part, easing in.
    fn on_open(&mut self, app: DockApp, icon: Option<Arc<RenderImage>>, jump: JumpApp, _serial: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let fixed = model::fixed_rows(app.launch.is_some(), ownpins::pin_from_app(&app).is_some(), app.pinned, app.windows.len());
        let windows = winops::refs(&app.windows);
        self.active = Some(Active { app, windows, jump, icon, menu: Menu::new(fixed), sections: Vec::new() });
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
        let px = (16.0 * cx.global::<Env>().scale).ceil() as u32;
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
                if let Some(img) = bgra.and_then(icons::to_image) {
                    let _ = this.update(cx, |me, cx| {
                        me.entry_icons.insert(key, img);
                        cx.notify();
                    });
                }
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
            RowId::Fixed(Fixed::Pin) => super::own_pins::set_pinned(a.app.clone(), true, cx),
            RowId::Fixed(Fixed::Unpin) => super::own_pins::set_pinned(a.app.clone(), false, cx),
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
                let icon = entry.and_then(|en| en.icon.as_ref()).and_then(|src| self.entry_icons.get(&plan::icon_key(src)).cloned());
                (icon, None, entry.map(|en| en.title.clone()).unwrap_or_default().into())
            }
            RowId::Fixed(f) => {
                let (glyph, label) = fixed_label(f, &a.app.name);
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

/// The glyph and the text of a fixed row. The name row's text is the app's name.
fn fixed_label(f: Fixed, app_name: &str) -> (Option<L>, String) {
    match f {
        Fixed::Open if app_name.trim().is_empty() => (Some(L::Play), t!("Uygulamayı başlat").to_string()),
        Fixed::Open => (Some(L::Play), app_name.to_string()),
        Fixed::Pin => (Some(L::Pin), t!("Dock'a sabitle").to_string()),
        Fixed::Unpin => (Some(L::PinOff), t!("Dock'tan kaldır").to_string()),
        Fixed::CloseWindow => (Some(L::X), t!("Pencereyi kapat").to_string()),
        Fixed::CloseAll(n) => (Some(L::X), format!("{} ({n})", t!("Tüm pencereleri kapat"))),
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

#[cfg(test)]
mod tests {
    use super::{Basis, Fixed, basis_changed, fixed_label};
    use rf_win::dock::layout::Rect;

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
