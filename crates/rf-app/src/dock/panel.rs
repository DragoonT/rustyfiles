//! The dock's panels: borderless windows on HyperGlass that open above an island (the Start panel above the left island, the control
//! centre above the right one). The arithmetic and the decisions are `rf_win::dock::panel` (placement, toggle, what closes a panel,
//! who gets the keyboard back, the 120 ms easing); this file puts them on windows. Each panel is a slot: its id, the island it sits
//! above and lines up with, its window, and the hooks that tell its view what is happening to it.
//!
//! WHAT WAS CHOSEN. A panel's window is created once, when the dock starts, and kept HIDDEN; opening is a move, a show and a bring
//! to the front, not a window creation. At rest that costs one hidden GPUI window per panel (its swap chain at the panel's size and
//! the view tree, not measured), no timer, no thread and no glass: the glass engine is attached when the panel opens and taken off
//! when it has closed, so a closed panel costs the engine nothing. While the glass starts (a few frames) the panel paints the plain
//! fill the popups paint (`glass_watch::panel_fill`).
//!
//! THE KEYBOARD. The islands are never activated; a panel is. On opening, the window that is in front is remembered; the panel is
//! brought to the front (the click that opened it was this process's own input, which is what lets it take the foreground) and its
//! view takes the keys. When the panel is put away with Escape or by its button the remembered window gets the foreground back (if
//! nobody else took it meanwhile); after a click elsewhere or a thing started from the panel the foreground is left alone.
//!
//! WHAT CLOSES IT. Escape; a mouse button going down outside the panel and outside its own island (the island's button toggles it);
//! another window taking the foreground; the dock hiding for a full-screen application; the monitor, its scale or its work area
//! changing (the islands move, the panel would be left behind); Alt+F4 (the window is kept, only put away); its button again. While a panel is open one
//! task looks every 50 ms at the foreground and the mouse buttons for the last two; while none is open nothing runs. Only one panel
//! is open at a time: opening the other puts the first away at once.
//!
//! A PANEL'S HEIGHT may change while it is open (the control centre gains a card when a media session starts): [`relayout`] places it
//! again, still 8 px above its island, so it grows upwards.
//!
//! ALL WIN32 CALLS THAT MOVE, SHOW, HIDE OR FOCUS A WINDOW run in a task of the foreground executor, outside any update of the app
//! (see the notes at the top of `host.rs`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use rf_win::dock::layout::Rect;
use rf_win::dock::panel::{self as geo, CloseCause, PanelId, Side, Toggle};

use super::backdrop::{self, IslandGlass};
use super::control_centre::CcView;
use super::host::{self, Kind};
use super::left::LeftIsland;
use super::start::StartView;

/// The Start panel's size, logical px.
pub const START_SIZE: (f32, f32) = (620.0, 680.0);

/// How often an open panel looks at the foreground and the mouse.
const WATCH_EVERY: Duration = Duration::from_millis(50);

/// What a panel's view is told, and what is asked of it. Each call is made from outside any update of the view.
trait Hooks {
    /// About to be shown: start from a clean state and begin easing in.
    fn on_open(&self, dark: bool, cx: &mut App);
    /// On the screen: the glass goes on, the keyboard goes to the view.
    fn on_shown(&self, dark: bool, cx: &mut App);
    /// Being put away: ease out, stop what was running for it.
    fn on_close(&self, cx: &mut App);
    /// Hidden: the glass comes off, nothing is kept alive.
    fn on_hidden(&self, cx: &mut App);
    /// The size it wants now, logical px (its height may depend on what it shows).
    fn size(&self, cx: &App) -> (f32, f32);
    /// The dock ends: the window goes with it.
    fn remove(&self, cx: &mut App);
}

struct Slot {
    id: PanelId,
    /// The island it sits above.
    kind: Kind,
    side: Side,
    hwnd: isize,
    window: WindowId,
    /// The island's glass: its tone (dark or light) is the panel's.
    island: Rc<RefCell<IslandGlass>>,
    hooks: Rc<dyn Hooks>,
}

struct PanelHost {
    slots: Vec<Slot>,
    /// The left island (it shows what is typed in the Start panel).
    left: Option<Entity<LeftIsland>>,
    start: Option<Entity<StartView>>,
    /// The panel that is open, if any (only one at a time).
    open: Option<PanelId>,
    /// Bumped at every open and close: a task of an earlier state sees that and does nothing.
    serial: u64,
    /// Where the open panel is, device px.
    rect: Rect,
    /// The window that had the foreground when the panel opened.
    previous: isize,
    opened_at: Instant,
    been_front: bool,
    mouse_was_down: bool,
    watcher: Option<Task<()>>,
    /// What is typed in the Start panel's field (the left island shows it).
    typed: String,
}

impl Global for PanelHost {}

impl PanelHost {
    fn slot(&self, id: PanelId) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }
}

fn host_global(cx: &mut App) -> &mut PanelHost {
    if !cx.has_global::<PanelHost>() {
        cx.set_global(PanelHost {
            slots: Vec::new(),
            left: None,
            start: None,
            open: None,
            serial: 0,
            rect: Rect::default(),
            previous: 0,
            opened_at: Instant::now(),
            been_front: false,
            mouse_was_down: false,
            watcher: None,
            typed: String::new(),
        });
    }
    cx.global_mut::<PanelHost>()
}

/// The hidden window of a panel: a transparent pop-up that is not shown yet.
fn open_hidden<V: Render + 'static>(
    initial: (f32, f32),
    display: Option<DisplayId>,
    near: Rect,
    scale: f32,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    cx: &mut App,
) -> Option<WindowHandle<V>> {
    let (x, y, _, _) = super::plan::logical(near, scale);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(x), px(y)), size(px(initial.0), px(initial.1))))),
        display_id: display,
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    match cx.open_window(opts, move |window, cx| cx.new(|cx| build(window, cx))) {
        Ok(h) => {
            // Alt+F4 in an open panel puts it away; it must not destroy the window (a destroyed window is never opened again)
            let _ = h.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.defer(|cx| close(CloseCause::WindowClose, cx));
                    false
                });
            });
            Some(h)
        }
        Err(e) => {
            log::error!("dock: cannot make a panel: {e:#}");
            None
        }
    }
}

// ---------------------------------------------------------------- the Start panel

struct StartHooks {
    handle: WindowHandle<StartView>,
}

impl Hooks for StartHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        // the input field's colours come from the component theme: make it the panel's tone
        apply_input_theme(dark, cx);
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
    }
    fn on_shown(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_shown(dark, window, cx));
    }
    fn on_close(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_close(window, cx));
    }
    fn on_hidden(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_hidden(window, cx));
    }
    fn size(&self, _cx: &App) -> (f32, f32) {
        START_SIZE
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

/// Makes the Start panel's window, hidden. Called once, from the dock's start-up.
pub fn init(island: Rc<RefCell<IslandGlass>>, left: Entity<LeftIsland>, display: Option<DisplayId>, scale: f32, near: Rect, cx: &mut App) {
    let isl = island.clone();
    let Some(handle) = open_hidden(START_SIZE, display, near, scale, move |window, cx| StartView::new(isl, window, cx), cx) else { return };
    let Ok(hwnd) = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) else { return };
    let Ok(view) = handle.entity(cx) else { return };
    let window = handle.window_id();
    let p = host_global(cx);
    p.left = Some(left);
    p.start = Some(view);
    p.slots.push(Slot { id: PanelId::START, kind: Kind::Left, side: Side::Left, hwnd, window, island, hooks: Rc::new(StartHooks { handle }) });
}

// ---------------------------------------------------------------- the control centre

struct CcHooks {
    handle: WindowHandle<CcView>,
    view: Entity<CcView>,
}

impl Hooks for CcHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
    }
    fn on_shown(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_shown(dark, window, cx));
    }
    fn on_close(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_close(window, cx));
    }
    fn on_hidden(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_hidden(window, cx));
    }
    fn size(&self, cx: &App) -> (f32, f32) {
        (super::cc_model::WIDTH, self.view.read(cx).height(cx))
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

/// Makes the control centre's window, hidden. Called once, from the dock's start-up, after the right island exists.
pub fn init_control_centre(island: Rc<RefCell<IslandGlass>>, display: Option<DisplayId>, scale: f32, near: Rect, cx: &mut App) {
    let isl = island.clone();
    let Some(handle) = open_hidden((super::cc_model::WIDTH, 300.0), display, near, scale, move |window, cx| CcView::new(isl, window, cx), cx) else { return };
    let Ok(hwnd) = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) else { return };
    let Ok(view) = handle.entity(cx) else { return };
    let window = handle.window_id();
    let p = host_global(cx);
    p.slots.push(Slot { id: PanelId::CONTROL_CENTRE, kind: Kind::Right, side: Side::Right, hwnd, window, island, hooks: Rc::new(CcHooks { handle, view }) });
}

// ---------------------------------------------------------------- opening, watching, closing

fn panel(cx: &App) -> Option<&PanelHost> {
    cx.try_global::<PanelHost>()
}

/// The Start panel's view (the file answers from the main app are handed to it).
pub fn view(cx: &App) -> Option<Entity<StartView>> {
    panel(cx).and_then(|p| p.start.clone())
}

/// What the user has typed in the open Start panel's field, for the left island's search field to show; `None` when nothing is typed.
pub fn typed_text(cx: &App) -> Option<String> {
    panel(cx).map(|p| p.typed.clone()).filter(|t| !t.is_empty())
}

/// The Start panel's field changed: the left island shows it.
pub fn set_typed(text: &str, cx: &mut App) {
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    if p.typed == text {
        return;
    }
    let left = p.left.clone();
    cx.global_mut::<PanelHost>().typed = text.to_string();
    if let Some(left) = left {
        left.update(cx, |_, cx| cx.notify());
    }
}

/// The island button's click: opens the Start panel, closes it when it is the open one.
pub fn toggle_start(cx: &mut App) {
    toggle(PanelId::START, cx);
}

/// The right island's network or volume symbol: opens the control centre, closes it when it is the open one.
pub fn toggle_control_centre(cx: &mut App) {
    toggle(PanelId::CONTROL_CENTRE, cx);
}

fn toggle(id: PanelId, cx: &mut App) {
    let open = panel(cx).and_then(|p| p.open);
    match geo::toggle(open, id) {
        Toggle::Open | Toggle::Switch => open_panel(id, cx),
        Toggle::Close => close(CloseCause::Button, cx),
    }
}

/// Whether the system's colours are dark for the open panel: its island's tone decides (it follows what is behind the glass).
fn dark_now(island: &RefCell<IslandGlass>) -> bool {
    island.borrow().palette(backdrop::now_ms()).mix < 0.5
}

fn open_panel(id: PanelId, cx: &mut App) {
    if host::is_fullscreen(cx) {
        return;
    }
    let Some((scale, work)) = host::monitor_work(cx) else { return };
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    let Some(slot) = p.slot(id) else { return };
    let (kind, side, hwnd, island, hooks) = (slot.kind, slot.side, slot.hwnd, slot.island.clone(), slot.hooks.clone());
    let other = p.open.filter(|o| *o != id);
    let Some(glass) = host::visible_glass_rect(kind, cx) else { return };
    let rect = geo::place(glass, hooks.size(cx), scale, side, work);
    // another panel is open: it goes at once (no easing: this one takes its place)
    if let Some(o) = other {
        put_away_now(o, cx);
    }
    let dark = dark_now(&island);
    let serial = {
        let p = cx.global_mut::<PanelHost>();
        p.serial += 1;
        // (a panel that is still easing out has no `open`, and the window in front is the panel itself then: nothing to remember)
        if p.open.is_none() {
            let front = rf_win::dock::foreground_root();
            if !p.slots.iter().any(|s| s.hwnd == front) {
                p.previous = front;
            }
        }
        p.open = Some(id);
        p.rect = rect;
        p.opened_at = Instant::now();
        p.been_front = false;
        p.mouse_was_down = geo::any_button_down();
        p.serial
    };
    hooks.on_open(dark, cx);
    // the rectangle again: opening may have changed what the panel wants to show (its height)
    let rect = geo::place(glass, hooks.size(cx), scale, side, work);
    cx.global_mut::<PanelHost>().rect = rect;
    // show, bring to the front, then glass (in this order: the engine attaches to a window that is on the screen)
    let h = hooks.clone();
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        geo::show_at(hwnd, rect);
        // (the quiet kind: no tap of the Alt key into the window that is in front; the click that opened the panel was ours)
        rf_win::window::bring_to_front_quietly(hwnd);
        cx.update(|cx| h.on_shown(dark, cx));
    })
    .detach();
    // the watch of an open panel
    let watcher = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(WATCH_EVERY).await;
            if !cx.update(|cx| watch_once(serial, cx)) {
                break;
            }
        }
    });
    cx.global_mut::<PanelHost>().watcher = Some(watcher);
}

/// A panel that changed its mind about its height while open: placed again above its island (it grows upwards). `height` is the new
/// one, logical px (the view's own height cannot be read from inside its own update).
pub fn relayout(id: PanelId, height: f32, cx: &mut App) {
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    if p.open != Some(id) {
        return;
    }
    let Some(slot) = p.slot(id) else { return };
    let (kind, side, hwnd) = (slot.kind, slot.side, slot.hwnd);
    let Some((scale, work)) = host::monitor_work(cx) else { return };
    let Some(glass) = host::visible_glass_rect(kind, cx) else { return };
    let rect = geo::place(glass, (super::cc_model::WIDTH, height), scale, side, work);
    if cx.global::<PanelHost>().rect == rect {
        return;
    }
    cx.global_mut::<PanelHost>().rect = rect;
    cx.spawn(async move |_| {
        geo::show_at(hwnd, rect);
    })
    .detach();
}

/// One look of the open panel at the foreground and the mouse. False when this watch is over (the panel closed or reopened).
fn watch_once(serial: u64, cx: &mut App) -> bool {
    let Some(p) = cx.try_global::<PanelHost>() else { return false };
    let Some(open) = p.open.filter(|_| p.serial == serial) else { return false };
    let Some(slot) = p.slot(open) else { return false };
    let (hwnd, kind) = (slot.hwnd, slot.kind);
    let (rect, opened_at, been_front, was_down) = (p.rect, p.opened_at, p.been_front, p.mouse_was_down);
    let keep = host::visible_glass_rect(kind, cx).unwrap_or_default();
    let fg = rf_win::dock::foreground_root();
    let front = fg == hwnd;
    let down = geo::any_button_down();
    let outside = geo::cursor().is_some_and(|c| geo::outside_click(was_down, down, c, rect, keep));
    {
        let p = cx.global_mut::<PanelHost>();
        p.been_front |= front;
        p.mouse_was_down = down;
    }
    if outside {
        close(CloseCause::ClickOutside, cx);
        return false;
    }
    if geo::lost_foreground(hwnd, fg, been_front || front, opened_at.elapsed().as_millis() as u64) {
        close(CloseCause::ForegroundLost, cx);
        return false;
    }
    true
}

/// Puts the open panel away: it eases out over 120 ms (at once with animations off), then the window is hidden and the glass taken
/// off. Does nothing when none is open.
pub fn close(cause: CloseCause, cx: &mut App) {
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    let Some(id) = p.open else { return };
    let Some(slot) = p.slot(id) else { return };
    let (hwnd, hooks) = (slot.hwnd, slot.hooks.clone());
    let (previous, left) = (p.previous, p.left.clone());
    let animate = crate::settings::animations();
    {
        let p = cx.global_mut::<PanelHost>();
        p.open = None;
        p.serial += 1;
        p.watcher = None;
        p.typed.clear();
    }
    if let Some(left) = left {
        left.update(cx, |_, cx| cx.notify());
    }
    hooks.on_close(cx);
    let restore = cause.restores_foreground();
    cx.spawn(async move |cx| {
        if animate {
            cx.background_executor().timer(Duration::from_millis(u64::from(geo::ANIM_MS) + 20)).await;
        }
        // opened again meanwhile (this panel itself): leave everything as it is. Another panel that opened has put this one away.
        let again = cx.update(|cx| panel(cx).is_some_and(|p| p.open == Some(id)));
        if again {
            return;
        }
        let fg = rf_win::dock::foreground_root();
        geo::hide(hwnd);
        cx.update(|cx| hooks.on_hidden(cx));
        if restore && let Some(target) = geo::restore_to(previous, fg, hwnd, geo::exists(previous)) {
            geo::give_back(target);
        }
    })
    .detach();
}

/// A panel goes at once: its view is told, its window hidden, nothing eased (another panel is taking its place).
fn put_away_now(id: PanelId, cx: &mut App) {
    let Some(slot) = panel(cx).and_then(|p| p.slot(id)) else { return };
    let (hwnd, hooks) = (slot.hwnd, slot.hooks.clone());
    {
        let p = cx.global_mut::<PanelHost>();
        p.open = None;
        p.watcher = None;
        p.typed.clear();
    }
    hooks.on_close(cx);
    geo::hide(hwnd);
    hooks.on_hidden(cx);
}

/// The dock ends: the panels' windows go with it.
pub fn shutdown(cx: &mut App) {
    if !cx.has_global::<PanelHost>() {
        return;
    }
    let p = cx.remove_global::<PanelHost>();
    for s in &p.slots {
        s.hooks.remove(cx);
    }
}

/// True when every window left is a panel's (or none is left): the islands are gone, the dock has nothing to show.
pub fn only_panels_left(cx: &App) -> bool {
    let mine: Vec<WindowId> = panel(cx).map(|p| p.slots.iter().map(|s| s.window).collect()).unwrap_or_default();
    cx.windows().iter().all(|w| mine.contains(&w.window_id()))
}

/// The component theme (the text field's colours) follows the panel's tone, not the app's theme setting: the dock process has no
/// other component to disagree.
fn apply_input_theme(dark: bool, cx: &mut App) {
    use gpui_kit::component::theme::{Theme, ThemeMode};
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::update(cx, |t| t.background = gpui_kit::transparent_black());
    if std::path::Path::new(r"C:\Windows\Fonts\SegUIVar.ttf").exists() {
        Theme::update(cx, |t| t.font_family = "Segoe UI Variable Text".into());
    }
}
